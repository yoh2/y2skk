use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;

use super::table::{KanaInput, KanaTable, KanaTransition};

#[derive(Debug, Error)]
pub enum KanaTableError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error at line {line}: {message}")]
    Parse { line: usize, message: String },
}

/// Parses a kana conversion table from its text source and returns a `KanaTable`.
pub fn parse_table(src: &str) -> Result<KanaTable, KanaTableError> {
    let mut transitions: Vec<KanaTransition> = Vec::new();
    let mut kata_overrides: Vec<KanaTransition> = Vec::new();
    let mut okuri_aliases: HashMap<char, char> = HashMap::new();

    #[derive(PartialEq, Eq)]
    enum Section {
        None,
        Meta,
        Trans,
        TransKatakana,
        OkuriAlias,
    }

    let mut section = Section::None;

    for (lineno, raw) in src.lines().enumerate() {
        let line_num = lineno + 1;

        // Strip comments (first unescaped `#`), then trailing spaces/CR/LF (but NOT tabs)
        let line = match find_comment_start(raw) {
            Some(pos) => &raw[..pos],
            None => raw,
        };
        let line = line.trim_end_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\n'));

        if line.is_empty() {
            continue;
        }

        // Section headers
        if line.starts_with('[') {
            match line {
                "[meta]" => {
                    section = Section::Meta;
                    continue;
                }
                "[trans]" => {
                    section = Section::Trans;
                    continue;
                }
                "[trans.katakana]" => {
                    section = Section::TransKatakana;
                    continue;
                }
                "[okuri_alias]" => {
                    section = Section::OkuriAlias;
                    continue;
                }
                other => {
                    return Err(KanaTableError::Parse {
                        line: line_num,
                        message: format!("unknown section: {other}"),
                    });
                }
            }
        }

        match section {
            Section::None => {
                return Err(KanaTableError::Parse {
                    line: line_num,
                    message: "content before any section header".into(),
                });
            }
            Section::Meta => {
                // [meta] uses "key = value" format; skip for now (read-only metadata)
                continue;
            }
            Section::Trans | Section::TransKatakana => {
                let t = parse_trans_line(line, line_num)?;
                if section == Section::Trans {
                    transitions.push(t);
                } else {
                    kata_overrides.push(t);
                }
            }
            Section::OkuriAlias => {
                parse_okuri_alias_line(line, line_num, &mut okuri_aliases)?;
            }
        }
    }

    Ok(KanaTable::new(transitions, kata_overrides, okuri_aliases))
}

/// Reads a kana conversion table from a file.
pub fn load_table(path: &Path) -> Result<KanaTable, KanaTableError> {
    let src = std::fs::read_to_string(path)?;
    parse_table(&src)
}

/// Returns the byte position of the first unescaped `#` in `raw`, or `None`.
/// A `#` preceded by an odd number of `\` characters is considered escaped and skipped.
fn find_comment_start(raw: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '#' => return Some(i),
            _ => {}
        }
    }
    None
}

/// Resolves backslash escape sequences in a field string.
///
/// Supported escapes:
/// - `\#`  → `#`
/// - `\*`  → `*`
/// - `\\`  → `\`
///
/// Any other `\x` sequence is a parse error, as is a trailing bare `\`.
fn unescape(s: &str, line_num: usize) -> Result<String, KanaTableError> {
    let err = |msg: &str| KanaTableError::Parse {
        line: line_num,
        message: msg.to_string(),
    };
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('#') => out.push('#'),
                Some('*') => out.push('*'),
                Some('\\') => out.push('\\'),
                Some(other) => return Err(err(&format!("unknown escape sequence: \\{other}"))),
                None => return Err(err("trailing backslash in field")),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// Parses the input field into a `KanaInput`.
/// - Bare `*` → `KanaInput::Wildcard`
/// - `\*` → `KanaInput::Char('*')` (literal asterisk)
/// - `\#` → `KanaInput::Char('#')` (literal hash)
/// - `\\` → `KanaInput::Char('\\')`
/// - Any other single character → `KanaInput::Char(c)`
fn parse_input_spec(s: &str, line_num: usize) -> Result<KanaInput, KanaTableError> {
    let err = |msg: &str| KanaTableError::Parse {
        line: line_num,
        message: msg.to_string(),
    };
    if s == "*" {
        return Ok(KanaInput::Wildcard);
    }
    let unescaped = unescape(s, line_num)?;
    let mut chars = unescaped.chars();
    let c = chars.next().ok_or_else(|| err("input field is empty"))?;
    if chars.next().is_some() {
        return Err(err(&format!(
            "input field must be a single character, got: {s:?}"
        )));
    }
    Ok(KanaInput::Char(c))
}

/// Parses the input field into a single `char`.
/// Used for `[okuri_alias]` entries where `KanaInput::Wildcard` is not meaningful.
fn parse_alias_char(s: &str, line_num: usize) -> Result<char, KanaTableError> {
    let err = |msg: &str| KanaTableError::Parse {
        line: line_num,
        message: msg.to_string(),
    };
    match parse_input_spec(s, line_num)? {
        KanaInput::Char(c) => Ok(c),
        KanaInput::Wildcard => Err(err("wildcard `*` is not valid in okuri_alias")),
    }
}

/// Parses a single line from `[trans]` or `[trans.katakana]`.
/// Format: `from TAB input TAB to TAB output` (4 tab-separated fields)
fn parse_trans_line(line: &str, line_num: usize) -> Result<KanaTransition, KanaTableError> {
    let err = |msg: &str| KanaTableError::Parse {
        line: line_num,
        message: msg.to_string(),
    };

    // Accept 3 or 4 tab-separated fields.
    // 3 fields: `from TAB input TAB to` — output is implicitly empty.
    // 4 fields: `from TAB input TAB to TAB output` — explicit output.
    let fields: Vec<&str> = line.splitn(4, '\t').collect();
    if fields.len() < 3 {
        return Err(err(&format!(
            "expected 3 or 4 tab-separated fields, got {}",
            fields.len()
        )));
    }

    let from = unescape(fields[0], line_num)?;
    let input = parse_input_spec(fields[1], line_num)?;
    let to = unescape(fields[2], line_num)?;
    let output = unescape(fields.get(3).copied().unwrap_or(""), line_num)?;

    Ok(KanaTransition {
        from,
        input,
        to,
        output,
    })
}

/// Parses a single line from `[okuri_alias]`.
/// Format: `from TAB to` (2 tab-separated fields)
fn parse_okuri_alias_line(
    line: &str,
    line_num: usize,
    map: &mut HashMap<char, char>,
) -> Result<(), KanaTableError> {
    let err = |msg: &str| KanaTableError::Parse {
        line: line_num,
        message: msg.to_string(),
    };

    let fields: Vec<&str> = line.splitn(2, '\t').collect();
    if fields.len() != 2 {
        return Err(err("expected 2 tab-separated fields in okuri_alias"));
    }

    let from = parse_alias_char(fields[0], line_num)?;
    let to = parse_alias_char(fields[1], line_num)?;
    map.insert(from, to);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_TABLE: &str = "\
[meta]
name = \"test\"

[trans]
\tk\tk\t
k\ta\t\tか
k\ti\t\tき
\tn\tn\t
n\t*\t\tん
n\ta\t\tな

[trans.katakana]

[okuri_alias]
c\tk
";

    #[test]
    fn test_parse_basic() {
        let table = parse_table(SIMPLE_TABLE).unwrap();

        let r = table.transition("", 'k', super::super::table::KanaMode::Hiragana);
        assert!(matches!(
            r,
            super::super::table::TransitionResult::Ok { .. }
        ));

        assert_eq!(table.okuri_key('c'), 'k');
        assert_eq!(table.okuri_key('k'), 'k');
    }

    #[test]
    fn test_wildcard_n() {
        let table = parse_table(SIMPLE_TABLE).unwrap();
        use super::super::table::KanaMode;

        // n + a → な (exact match takes priority over wildcard)
        let r = table.transition("n", 'a', KanaMode::Hiragana);
        assert_eq!(
            r,
            super::super::table::TransitionResult::Ok {
                output: "な".into(),
                next_state: "".into(),
            }
        );

        // n + k → ん via wildcard; 'k' is retried from the start state (OkRetry)
        let r = table.transition("n", 'k', KanaMode::Hiragana);
        assert_eq!(
            r,
            super::super::table::TransitionResult::OkRetry {
                output: "ん".into(),
                retry: 'k',
            }
        );
    }

    #[test]
    fn test_literal_hash_input() {
        let src = "[trans]\n\t\\#\t\t＃\n";
        let table = parse_table(src).unwrap();
        use super::super::table::KanaMode;
        let r = table.transition("", '#', KanaMode::Hiragana);
        assert_eq!(
            r,
            super::super::table::TransitionResult::Ok {
                output: "＃".into(),
                next_state: "".into(),
            }
        );
    }

    #[test]
    fn test_literal_asterisk_input() {
        let src = "[trans]\n\t\\*\t\t＊\n";
        let table = parse_table(src).unwrap();
        use super::super::table::KanaMode;
        // '\*' is now a literal '*', not a wildcard — 'x' should be NoMatch
        let r = table.transition("", 'x', KanaMode::Hiragana);
        assert_eq!(
            r,
            super::super::table::TransitionResult::NoMatch {
                flush: "".into(),
                retry: None,
            }
        );
        // '*' key itself should produce "＊"
        let r = table.transition("", '*', KanaMode::Hiragana);
        assert_eq!(
            r,
            super::super::table::TransitionResult::Ok {
                output: "＊".into(),
                next_state: "".into(),
            }
        );
    }

    #[test]
    fn test_literal_backslash_input() {
        let src = "[trans]\n\t\\\\\t\tＸ\n";
        let table = parse_table(src).unwrap();
        use super::super::table::KanaMode;
        let r = table.transition("", '\\', KanaMode::Hiragana);
        assert_eq!(
            r,
            super::super::table::TransitionResult::Ok {
                output: "Ｘ".into(),
                next_state: "".into(),
            }
        );
    }

    #[test]
    fn test_comment_with_escaped_hash() {
        // "\#" in a line should NOT be treated as a comment start.
        let src = "[trans]\n\t\\#\t\t＃\t# this comment should not affect parsing\n";
        let table = parse_table(src).unwrap();
        use super::super::table::KanaMode;
        let r = table.transition("", '#', KanaMode::Hiragana);
        assert_eq!(
            r,
            super::super::table::TransitionResult::Ok {
                output: "＃".into(),
                next_state: "".into(),
            }
        );
    }

    #[test]
    fn test_unknown_escape_errors() {
        let src = "[trans]\n\t\\z\t\tＺ\n";
        assert!(parse_table(src).is_err());
    }

    #[test]
    fn test_trailing_backslash_errors() {
        let src = "[trans]\n\t\\\t\tＸ\n";
        assert!(parse_table(src).is_err());
    }

    #[test]
    fn test_wildcard_in_okuri_alias_errors() {
        let src = "[okuri_alias]\n*\tk\n";
        assert!(parse_table(src).is_err());
    }
}
