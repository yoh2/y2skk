/// Classification and serialization of Lisp-form dictionary candidates.
///
/// SKK-JISYO files may contain candidates that start with `(`, indicating an
/// Emacs Lisp S-expression.  The only currently supported directive is
/// `skk-ignore-dic-word`; all other Lisp forms are classified as `Unknown` and
/// passed through opaquely (displayed to the user, round-tripped unchanged).
use crate::dict::entry::LispForm;

/// Classifies a candidate word string.
///
/// Returns `Some(LispForm)` if the string begins with `(` (a Lisp-form
/// candidate), or `None` if it is a plain literal string.
pub fn classify(word: &str) -> Option<LispForm> {
    if !word.starts_with('(') {
        return None;
    }
    if let Some(words) = parse_ignore_dic_word(word) {
        Some(LispForm::IgnoreDicWord(words))
    } else {
        Some(LispForm::Unknown)
    }
}

/// Attempts to parse `(skk-ignore-dic-word "w1" "w2" ...)` and returns the
/// list of words to ignore, or `None` if the string does not match.
///
/// Handles `\"` and `\\` escape sequences inside quoted strings.
fn parse_ignore_dic_word(s: &str) -> Option<Vec<String>> {
    let s = s.trim();

    // Must start with '(' and end with ')'
    let inner = s.strip_prefix('(')?.strip_suffix(')')?;
    let inner = inner.trim();

    // First token must be the keyword
    let rest = inner.strip_prefix("skk-ignore-dic-word")?;
    // After the keyword there must be whitespace (or end of expression)
    if !rest.is_empty() && !rest.starts_with(|c: char| c.is_ascii_whitespace()) {
        return None;
    }
    let rest = rest.trim_start();

    // Parse zero or more quoted string literals
    let mut words = Vec::new();
    let mut chars = rest.chars().peekable();

    loop {
        // Skip whitespace
        while chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            chars.next();
        }
        match chars.peek() {
            None => break,
            Some('"') => {
                chars.next(); // consume opening `"`
                let mut word = String::new();
                loop {
                    match chars.next() {
                        None => return None, // unterminated string
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('"') => word.push('"'),
                            Some('\\') => word.push('\\'),
                            _ => return None, // unsupported escape
                        },
                        Some(c) => word.push(c),
                    }
                }
                words.push(word);
            }
            Some(_) => return None, // unexpected token
        }
    }

    Some(words)
}

/// Renders a `LispForm::IgnoreDicWord` back to its S-expression string.
/// Used when re-serializing a user dictionary that contains such an entry.
pub fn render_ignore_dic_word(words: &[String]) -> String {
    let quoted: Vec<String> = words
        .iter()
        .map(|w| {
            let escaped = w.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        })
        .collect();
    format!("(skk-ignore-dic-word {})", quoted.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_literal() {
        assert!(classify("普通の単語").is_none());
        assert!(classify("テスト").is_none());
        assert!(classify("").is_none());
    }

    #[test]
    fn test_classify_ignore_dic_word_empty() {
        let form = classify("(skk-ignore-dic-word)");
        assert_eq!(form, Some(LispForm::IgnoreDicWord(vec![])));
    }

    #[test]
    fn test_classify_ignore_dic_word_single() {
        let form = classify("(skk-ignore-dic-word \"無視\")");
        assert_eq!(
            form,
            Some(LispForm::IgnoreDicWord(vec!["無視".to_string()]))
        );
    }

    #[test]
    fn test_classify_ignore_dic_word_multiple() {
        let form = classify("(skk-ignore-dic-word \"foo\" \"bar\" \"baz\")");
        assert_eq!(
            form,
            Some(LispForm::IgnoreDicWord(vec![
                "foo".to_string(),
                "bar".to_string(),
                "baz".to_string(),
            ]))
        );
    }

    #[test]
    fn test_classify_ignore_dic_word_escape() {
        let form = classify("(skk-ignore-dic-word \"say \\\"hi\\\"\" \"back\\\\slash\")");
        assert_eq!(
            form,
            Some(LispForm::IgnoreDicWord(vec![
                "say \"hi\"".to_string(),
                "back\\slash".to_string(),
            ]))
        );
    }

    #[test]
    fn test_classify_unknown_lisp() {
        let form = classify("(concat \"a\" \"b\")");
        assert_eq!(form, Some(LispForm::Unknown));
    }

    #[test]
    fn test_classify_unknown_no_keyword_match() {
        // Close keyword but not identical
        let form = classify("(skk-ignore-dic-word-extra \"x\")");
        assert_eq!(form, Some(LispForm::Unknown));
    }

    #[test]
    fn test_render_roundtrip_single() {
        let original = "(skk-ignore-dic-word \"無視\")";
        let parsed = parse_ignore_dic_word(original).unwrap();
        let rendered = render_ignore_dic_word(&parsed);
        assert_eq!(rendered, original);
    }

    #[test]
    fn test_render_roundtrip_multiple() {
        let original = "(skk-ignore-dic-word \"foo\" \"bar\")";
        let parsed = parse_ignore_dic_word(original).unwrap();
        let rendered = render_ignore_dic_word(&parsed);
        assert_eq!(rendered, original);
    }

    #[test]
    fn test_render_escaping() {
        let words = vec!["say \"hi\"".to_string(), "back\\slash".to_string()];
        let rendered = render_ignore_dic_word(&words);
        assert_eq!(
            rendered,
            "(skk-ignore-dic-word \"say \\\"hi\\\"\" \"back\\\\slash\")"
        );
    }
}
