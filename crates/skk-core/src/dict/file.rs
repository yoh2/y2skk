use std::collections::HashMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use super::entry::{Candidate, DictEntry, DictError, LispForm};
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
    pub fn load(
        path: impl AsRef<Path>,
        encoding: DictEncoding,
        priority: i32,
    ) -> Result<Self, DictError> {
        let path = path.as_ref().to_path_buf();
        let src = read_to_utf8(&path, encoding)?;
        let entries = parse_skk_dict(&src)?;
        Ok(Self {
            path,
            entries,
            priority,
        })
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

    /// Returns all okuri-nashi headwords starting with `prefix` (excluding
    /// `prefix` itself) in sorted order.
    fn complete(&self, prefix: &str) -> Vec<String> {
        let mut results: Vec<String> = self
            .entries
            .iter()
            .filter(|(midashi, okuri_map)| {
                midashi.starts_with(prefix)
                    && midashi.as_str() != prefix
                    && okuri_map.contains_key(&None)
            })
            .map(|(midashi, _)| midashi.clone())
            .collect();
        results.sort();
        results
    }
}

// ── UserDict (read-write) ─────────────────────────────────────────────────────

/// Writable user dictionary, always stored as UTF-8.
///
/// The daemon owns the user dict exclusively; adapters never write to it directly.
/// Headwords are stored in an IndexMap ordered oldest-first; `learn()` moves the
/// used headword to the end so `complete()` (which iterates in reverse) returns
/// the most-recently-used headwords first.
pub struct UserDict {
    path: PathBuf,
    /// midashi → okuri → candidates (IndexMap: oldest first, most-recently-used last)
    entries: UserEntryMap,
    dirty: bool,
}

impl UserDict {
    /// Creates an empty in-memory user dict pointed at `path` (not yet saved).
    pub fn empty(path: PathBuf) -> Self {
        Self {
            path,
            entries: IndexMap::new(),
            dirty: false,
        }
    }

    /// Loads the user dict from disk, creating an empty dict if the file does not exist.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DictError> {
        let path = path.as_ref().to_path_buf();
        let entries = if path.exists() {
            let src = std::fs::read_to_string(&path)?;
            parse_skk_dict_ordered(&src)?
        } else {
            IndexMap::new()
        };
        Ok(Self {
            path,
            entries,
            dirty: false,
        })
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
        let content = serialize_user_dict(&self.entries);
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
    /// Also moves the headword to the end of the IndexMap so that `complete()`
    /// (which iterates in reverse) returns the most-recently-used headwords first.
    ///
    /// If the user dict contains a `(skk-ignore-dic-word ...)` entry for the same
    /// headword, any learned word is removed from that ignore list.  An ignore
    /// entry whose list becomes empty is dropped entirely.
    fn learn(&mut self, entry: DictEntry) -> Result<(), DictError> {
        // Remove and re-insert at the end to record recency.
        let mut okuri_map = self
            .entries
            .shift_remove(&entry.midashi)
            .unwrap_or_default();

        // Collect the plain words being learned so we can remove them from any
        // skk-ignore-dic-word directives in the same bucket.
        let learned_words: Vec<String> = entry
            .candidates
            .iter()
            .filter(|c| c.lisp_form.is_none())
            .map(|c| c.word.clone())
            .collect();

        let list = okuri_map.entry(entry.okuri).or_default();
        for new_cand in entry.candidates.into_iter().rev() {
            // Remove any existing occurrence of the same word.
            list.retain(|c| c.word != new_cand.word);
            // Prepend to give it highest priority.
            list.insert(0, new_cand);
        }

        // Remove learned words from any skk-ignore-dic-word entries in this
        // bucket, and drop the directive entirely when its list becomes empty.
        for cand in list.iter_mut() {
            if let Some(LispForm::IgnoreDicWord(words)) = &mut cand.lisp_form {
                words.retain(|w| !learned_words.contains(w));
                cand.word = super::lisp::render_ignore_dic_word(words);
            }
        }
        list.retain(
            |c| !matches!(&c.lisp_form, Some(LispForm::IgnoreDicWord(ws)) if ws.is_empty()),
        );

        // Re-insert at the end (= most recently used position).
        self.entries.insert(entry.midashi, okuri_map);
        self.dirty = true;
        Ok(())
    }

    /// Removes the candidate whose word equals `word` from the user dict.
    /// Cleans up empty okuri buckets and empty midashi entries so the on-disk
    /// representation stays compact after `save()`.
    fn purge(&mut self, midashi: &str, okuri: Option<&str>, word: &str) -> Result<bool, DictError> {
        let Some(okuri_map) = self.entries.get_mut(midashi) else {
            return Ok(false);
        };
        let key = okuri.map(|s| s.to_string());
        let Some(candidates) = okuri_map.get_mut(&key) else {
            return Ok(false);
        };

        let before = candidates.len();
        candidates.retain(|c| c.word != word);
        let removed = candidates.len() != before;

        if candidates.is_empty() {
            okuri_map.remove(&key);
        }
        if okuri_map.is_empty() {
            self.entries.shift_remove(midashi);
        }
        if removed {
            self.dirty = true;
        }
        Ok(removed)
    }

    fn priority(&self) -> i32 {
        // User dict has highest priority.
        i32::MAX
    }

    /// Returns okuri-nashi headwords starting with `prefix` (excluding `prefix`
    /// itself) in most-recently-used-first order (reverse IndexMap iteration).
    fn complete(&self, prefix: &str) -> Vec<String> {
        self.entries
            .iter()
            .rev()
            .filter(|(midashi, okuri_map)| {
                midashi.starts_with(prefix)
                    && midashi.as_str() != prefix
                    && okuri_map.contains_key(&None)
            })
            .map(|(midashi, _)| midashi.clone())
            .collect()
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────────

type EntryMap = HashMap<String, HashMap<Option<String>, Vec<Candidate>>>;
type UserEntryMap = IndexMap<String, HashMap<Option<String>, Vec<Candidate>>>;

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
        // The trailing ASCII letter is the okurigana consonant only when the midashi
        // contains kana characters.  A purely ASCII midashi (abbrev mode entry such as
        // "is" or "define") must not be split.
        if last.is_ascii_alphabetic() && raw.chars().any(|c| !c.is_ascii()) {
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
        // Lisp-form candidates start with `(`.  They may contain `;` inside
        // string literals, so we must NOT apply the annotation `;` split.
        let cand = if field.starts_with('(') {
            let form = crate::dict::lisp::classify(field);
            Candidate::lisp(field, form.unwrap_or(crate::dict::entry::LispForm::Unknown))
        } else if let Some(semi) = field.find(';') {
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

// ── Parsing (ordered, for UserDict) ──────────────────────────────────────────

/// Like `parse_skk_dict` but inserts into an IndexMap, preserving file line order.
/// This ensures that the initial load reflects the serialized recency ordering.
fn parse_skk_dict_ordered(src: &str) -> Result<UserEntryMap, DictError> {
    let mut map: UserEntryMap = IndexMap::new();

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

// ── Serialisation ─────────────────────────────────────────────────────────────

/// Serializes a UserDict (IndexMap) to UTF-8 SKK format, preserving IndexMap
/// order (oldest first) so that recency is correctly restored on next load.
fn serialize_user_dict(map: &UserEntryMap) -> String {
    let mut okuri_ari: Vec<String> = Vec::new();
    let mut okuri_nasi: Vec<String> = Vec::new();

    for (midashi, okuri_map) in map {
        for (okuri, candidates) in okuri_map {
            let cands: String = candidates
                .iter()
                .map(|c| {
                    // Lisp-form candidates: re-render the S-expression; annotations
                    // are not supported for Lisp forms to avoid `;` ambiguity.
                    if let Some(form) = &c.lisp_form {
                        use crate::dict::entry::LispForm;
                        match form {
                            LispForm::IgnoreDicWord(words) => {
                                crate::dict::lisp::render_ignore_dic_word(words)
                            }
                            LispForm::Unknown => c.word.clone(),
                        }
                    } else if let Some(ann) = &c.annotation {
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

    // Do NOT sort: preserve IndexMap order (oldest-first) so that complete()
    // returns most-recently-used headwords first on the next load.
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
            entries: IndexMap::new(),
            dirty: false,
        };

        udict
            .learn(DictEntry {
                midashi: "あ".into(),
                okuri: Some("k".into()),
                candidates: vec![Candidate::new("明")],
            })
            .unwrap();
        assert!(udict.dirty);

        // Learning again promotes to front
        udict
            .learn(DictEntry {
                midashi: "あ".into(),
                okuri: Some("k".into()),
                candidates: vec![Candidate::new("空")],
            })
            .unwrap();

        let entry = udict.lookup("あ", Some("k")).unwrap();
        assert_eq!(entry.candidates[0].word, "空");
        assert_eq!(entry.candidates[1].word, "明");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let entries = parse_skk_dict_ordered(SAMPLE).unwrap();
        let serialized = serialize_user_dict(&entries);
        let reparsed = parse_skk_dict_ordered(&serialized).unwrap();

        // Check key entries survived the roundtrip
        assert!(reparsed.get("あ").unwrap().get(&Some("k".into())).is_some());
        assert!(reparsed.get("てすと").unwrap().get(&None).is_some());
    }

    #[test]
    fn test_learn_removes_word_from_ignore_dic_word() {
        // When a word listed in skk-ignore-dic-word is explicitly learned, it
        // should be removed from the directive.
        let ignore_cand = Candidate::lisp(
            "(skk-ignore-dic-word \"無視\" \"除外\")",
            LispForm::IgnoreDicWord(vec!["無視".into(), "除外".into()]),
        );
        let mut udict = UserDict {
            path: PathBuf::from("/tmp/y2skk_test.dict"),
            entries: {
                let mut m = IndexMap::new();
                let mut inner = HashMap::new();
                inner.insert(None, vec![Candidate::new("普通"), ignore_cand]);
                m.insert("むし".to_string(), inner);
                m
            },
            dirty: false,
        };

        // Learn one of the ignored words.
        udict
            .learn(DictEntry {
                midashi: "むし".into(),
                okuri: None,
                candidates: vec![Candidate::new("無視")],
            })
            .unwrap();

        let entry = udict.lookup("むし", None).unwrap();
        // "無視" should now be at the front.
        assert_eq!(entry.candidates[0].word, "無視");
        // The ignore directive should still exist but without "無視".
        let ignore = entry
            .candidates
            .iter()
            .find(|c| matches!(&c.lisp_form, Some(LispForm::IgnoreDicWord(_))));
        let ignore = ignore.expect("IgnoreDicWord entry should still exist");
        if let Some(LispForm::IgnoreDicWord(ws)) = &ignore.lisp_form {
            assert_eq!(ws, &["除外".to_string()]);
        }
    }

    #[test]
    fn test_learn_drops_empty_ignore_dic_word() {
        // When all words in skk-ignore-dic-word are learned, the entry is removed.
        let ignore_cand = Candidate::lisp(
            "(skk-ignore-dic-word \"唯一\")",
            LispForm::IgnoreDicWord(vec!["唯一".into()]),
        );
        let mut udict = UserDict {
            path: PathBuf::from("/tmp/y2skk_test.dict"),
            entries: {
                let mut m = IndexMap::new();
                let mut inner = HashMap::new();
                inner.insert(None, vec![ignore_cand]);
                m.insert("ゆいいつ".to_string(), inner);
                m
            },
            dirty: false,
        };

        udict
            .learn(DictEntry {
                midashi: "ゆいいつ".into(),
                okuri: None,
                candidates: vec![Candidate::new("唯一")],
            })
            .unwrap();

        let entry = udict.lookup("ゆいいつ", None).unwrap();
        // The ignore directive should be gone.
        assert!(!entry.candidates.iter().any(|c| c.lisp_form.is_some()));
        assert_eq!(entry.candidates[0].word, "唯一");
    }

    #[test]
    fn test_encoding_from_str() {
        assert_eq!(DictEncoding::from_str("euc-jp"), DictEncoding::EucJp);
        assert_eq!(DictEncoding::from_str("EUC-JP"), DictEncoding::EucJp);
        assert_eq!(DictEncoding::from_str("utf-8"), DictEncoding::Utf8);
        assert_eq!(DictEncoding::from_str("utf8"), DictEncoding::Utf8);
    }
}
