use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::entry::{Candidate, DictEntry, DictError};
use super::traits::DictionaryProvider;

// ── Encoding ──────────────────────────────────────────────────────────────────

/// Character encoding of a dictionary file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DictEncoding {
    #[default]
    Utf8,
    EucJp,
}

impl DictEncoding {
    /// Parses an encoding name string (case-insensitive).
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "euc-jp" | "eucjp" | "euc_jp" => Self::EucJp,
            _ => Self::Utf8,
        }
    }
}

/// Reads a file and decodes it to UTF-8, handling both UTF-8 and EUC-JP.
fn read_to_utf8(path: &Path, encoding: DictEncoding) -> Result<String, DictError> {
    match encoding {
        DictEncoding::Utf8 => Ok(std::fs::read_to_string(path)?),
        DictEncoding::EucJp => {
            let bytes = std::fs::read(path)?;
            let (cow, _, had_errors) = encoding_rs::EUC_JP.decode(&bytes);
            if had_errors {
                tracing::warn!(
                    "EUC-JP decoding errors in {}; some characters may be incorrect",
                    path.display()
                );
            }
            Ok(cow.into_owned())
        }
    }
}

// ── FileDict (read-only) ──────────────────────────────────────────────────────

/// SKK dictionary loaded from a file (UTF-8 or EUC-JP, read-only).
///
/// File format (standard SKK dictionary):
/// - Lines starting with `;` are comments.
/// - `;;; okuri-ari entries.` / `;;; okuri-nasi entries.` mark sections.
/// - Entry lines: `midashi /cand1/cand2/.../`
///   - Okuri-ari midashi ends with the okurigana consonant (e.g. `あk /明/`).
///   - Candidate annotations follow `;` inside the slash-delimited field.
pub struct FileDict {
    path: PathBuf,
    /// midashi → okuri → candidates
    entries: HashMap<String, HashMap<Option<String>, Vec<Candidate>>>,
    priority: i32,
}

impl FileDict {
    /// Loads a dictionary file with the specified encoding.
    pub fn load(path: impl AsRef<Path>, encoding: DictEncoding, priority: i32) -> Result<Self, DictError> {
        let path = path.as_ref().to_path_buf();
        let src = read_to_utf8(&path, encoding)?;
        let entries = parse_skk_dict(&src)?;
        Ok(Self { path, entries, priority })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl DictionaryProvider for FileDict {
    fn lookup(&self, midashi: &str, okuri: Option<&str>) -> Option<DictEntry> {
        let okuri_map = self.entries.get(midashi)?;
        let key = okuri.map(|s| s.to_string());
        let candidates = okuri_map.get(&key)?.clone();
        Some(DictEntry {
            midashi: midashi.to_string(),
            okuri: okuri.map(|s| s.to_string()),
            candidates,
        })
    }

    fn learn(&mut self, _entry: DictEntry) -> Result<(), DictError> {
        Err(DictError::ReadOnly)
    }

    fn priority(&self) -> i32 {
        self.priority
    }
}

// ── UserDict (read-write) ─────────────────────────────────────────────────────

/// Writable user dictionary, always stored as UTF-8.
///
/// The daemon owns the user dict exclusively; adapters never write to it directly.
/// Learned entries are prepended (highest priority) within their section.
pub struct UserDict {
    path: PathBuf,
    /// midashi → okuri → candidates (ordered: most-recently-learned first)
    entries: HashMap<String, HashMap<Option<String>, Vec<Candidate>>>,
    dirty: bool,
}

impl UserDict {
    /// Creates an empty in-memory user dict pointed at `path` (not yet saved).
    pub fn empty(path: PathBuf) -> Self {
        Self { path, entries: HashMap::new(), dirty: false }
    }

    /// Loads the user dict from disk, creating an empty dict if the file does not exist.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DictError> {
        let path = path.as_ref().to_path_buf();
        let entries = if path.exists() {
            let src = std::fs::read_to_string(&path)?;
            parse_skk_dict(&src)?
        } else {
            HashMap::new()
        };
        Ok(Self { path, entries, dirty: false })
    }

    /// Persists any changes back to disk (UTF-8 SKK format).
    pub fn save(&mut self) -> Result<(), DictError> {
        if !self.dirty {
            return Ok(());
        }
        // Ensure parent directory exists.
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serialize_skk_dict(&self.entries);
        std::fs::write(&self.path, content)?;
        self.dirty = false;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

impl DictionaryProvider for UserDict {
    fn lookup(&self, midashi: &str, okuri: Option<&str>) -> Option<DictEntry> {
        let okuri_map = self.entries.get(midashi)?;
        let key = okuri.map(|s| s.to_string());
        let candidates = okuri_map.get(&key)?.clone();
        Some(DictEntry {
            midashi: midashi.to_string(),
            okuri: okuri.map(|s| s.to_string()),
            candidates,
        })
    }

    /// Learns a new conversion, promoting it to the front of the candidate list.
    fn learn(&mut self, entry: DictEntry) -> Result<(), DictError> {
        let okuri_map = self.entries.entry(entry.midashi).or_default();
        let list = okuri_map.entry(entry.okuri).or_default();

        for new_cand in entry.candidates.into_iter().rev() {
            // Remove any existing occurrence of the same word.
            list.retain(|c| c.word != new_cand.word);
            // Prepend to give it highest priority.
            list.insert(0, new_cand);
        }

        self.dirty = true;
        Ok(())
    }

    fn priority(&self) -> i32 {
        // User dict has highest priority.
        i32::MAX
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────────

type EntryMap = HashMap<String, HashMap<Option<String>, Vec<Candidate>>>;

fn parse_skk_dict(src: &str) -> Result<EntryMap, DictError> {
    let mut map: EntryMap = HashMap::new();

    for (lineno, line) in src.lines().enumerate() {
        let line_num = lineno + 1;

        if line.starts_with(';') {
            continue;
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some(slash_pos) = line.find(" /") else {
            return Err(DictError::Parse {
                line: line_num,
                message: format!("missing ` /` separator: {line:?}"),
            });
        };

        let midashi_raw = &line[..slash_pos];
        let cands_raw = &line[slash_pos + 2..];

        let (midashi, okuri) = split_midashi_okuri(midashi_raw);
        let candidates = parse_candidates(cands_raw, line_num)?;

        map.entry(midashi.to_string())
            .or_default()
            .entry(okuri.map(|s| s.to_string()))
            .or_default()
            .extend(candidates);
    }

    Ok(map)
}

fn split_midashi_okuri(raw: &str) -> (&str, Option<&str>) {
    let bytes = raw.as_bytes();
    if let Some(&last) = bytes.last() {
        if last.is_ascii_alphabetic() {
            let split = raw.len() - 1;
            return (&raw[..split], Some(&raw[split..]));
        }
    }
    (raw, None)
}

fn parse_candidates(s: &str, line_num: usize) -> Result<Vec<Candidate>, DictError> {
    let mut candidates = Vec::new();
    for field in s.split('/') {
        if field.is_empty() {
            continue;
        }
        let cand = if let Some(semi) = field.find(';') {
            Candidate::with_annotation(&field[..semi], &field[semi + 1..])
        } else {
            Candidate::new(field)
        };
        if cand.word.is_empty() {
            return Err(DictError::Parse {
                line: line_num,
                message: format!("empty candidate word in field: {field:?}"),
            });
        }
        candidates.push(cand);
    }
    Ok(candidates)
}

// ── Serialisation ─────────────────────────────────────────────────────────────

fn serialize_skk_dict(map: &EntryMap) -> String {
    let mut okuri_ari: Vec<String> = Vec::new();
    let mut okuri_nasi: Vec<String> = Vec::new();

    for (midashi, okuri_map) in map {
        for (okuri, candidates) in okuri_map {
            let cands: String = candidates
                .iter()
                .map(|c| {
                    if let Some(ann) = &c.annotation {
                        format!("{};{}", c.word, ann)
                    } else {
                        c.word.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join("/");

            let line = if let Some(ok) = okuri {
                format!("{midashi}{ok} /{cands}/")
            } else {
                format!("{midashi} /{cands}/")
            };

            if okuri.is_some() {
                okuri_ari.push(line);
            } else {
                okuri_nasi.push(line);
            }
        }
    }

    okuri_ari.sort();
    okuri_nasi.sort();

    format!(
        ";; y2skk user dictionary\n\
         ;;; okuri-ari entries.\n\
         {}\n\
         ;;; okuri-nasi entries.\n\
         {}\n",
        okuri_ari.join("\n"),
        okuri_nasi.join("\n"),
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
;; SKK-JISYO.test
;;; okuri-ari entries.
あk /明/空/
;;; okuri-nasi entries.
てすと /テスト/test/
";

    #[test]
    fn test_parse() {
        let entries = parse_skk_dict(SAMPLE).unwrap();
        let okuri_ari = entries.get("あ").unwrap().get(&Some("k".into())).unwrap();
        assert_eq!(okuri_ari[0].word, "明");
        assert_eq!(okuri_ari[1].word, "空");

        let nasi = entries.get("てすと").unwrap().get(&None).unwrap();
        assert_eq!(nasi[0].word, "テスト");
        assert_eq!(nasi[1].word, "test");
    }

    #[test]
    fn test_lookup() {
        let entries = parse_skk_dict(SAMPLE).unwrap();
        let dict = FileDict {
            path: PathBuf::from("test"),
            entries,
            priority: 0,
        };

        let result = dict.lookup("あ", Some("k")).unwrap();
        assert_eq!(result.candidates[0].word, "明");
        assert!(dict.lookup("あ", None).is_none());
    }

    #[test]
    fn test_user_dict_learn() {
        let mut udict = UserDict {
            path: PathBuf::from("/tmp/y2skk_test.dict"),
            entries: HashMap::new(),
            dirty: false,
        };

        udict.learn(DictEntry {
            midashi: "あ".into(),
            okuri: Some("k".into()),
            candidates: vec![Candidate::new("明")],
        }).unwrap();
        assert!(udict.dirty);

        // Learning again promotes to front
        udict.learn(DictEntry {
            midashi: "あ".into(),
            okuri: Some("k".into()),
            candidates: vec![Candidate::new("空")],
        }).unwrap();

        let entry = udict.lookup("あ", Some("k")).unwrap();
        assert_eq!(entry.candidates[0].word, "空");
        assert_eq!(entry.candidates[1].word, "明");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let entries = parse_skk_dict(SAMPLE).unwrap();
        let serialized = serialize_skk_dict(&entries);
        let reparsed = parse_skk_dict(&serialized).unwrap();

        // Check key entries survived the roundtrip
        assert!(reparsed.get("あ").unwrap().get(&Some("k".into())).is_some());
        assert!(reparsed.get("てすと").unwrap().get(&None).is_some());
    }

    #[test]
    fn test_encoding_from_str() {
        assert_eq!(DictEncoding::from_str("euc-jp"), DictEncoding::EucJp);
        assert_eq!(DictEncoding::from_str("EUC-JP"), DictEncoding::EucJp);
        assert_eq!(DictEncoding::from_str("utf-8"), DictEncoding::Utf8);
        assert_eq!(DictEncoding::from_str("utf8"), DictEncoding::Utf8);
    }
}
