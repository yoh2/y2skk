/// Classification, evaluation and serialization of Lisp-form dictionary
/// candidates.
///
/// SKK-JISYO files may contain candidates that start with `(`, indicating an
/// Emacs Lisp S-expression.  Two forms are understood:
///
/// - `(concat "..." ...)` — a string built from literals.  It is the standard
///   way to store a candidate or annotation that contains characters the
///   dictionary syntax reserves (`/` and `;`, written as `\057` and `\073`).
///   `parse_concat` evaluates it to a plain string at load time and
///   `quote_concat` produces it at save time.
/// - `(skk-ignore-dic-word "w1" ...)` — a directive, never displayed.
///
/// Any other form is classified as `Unknown`: it is hidden from the user and
/// round-tripped unchanged when the user dictionary is saved.
use std::iter::Peekable;
use std::str::Chars;

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

/// Reads one Emacs Lisp string literal from `chars`, which must be positioned
/// just after the opening `"`.  Consumes the closing `"`.
///
/// Supported escapes: `\\`, `\"`, `\n`, `\t`, `\r`, and octal `\ooo` (one to
/// three digits, e.g. `\057` for `/`).  Returns `None` on an unterminated
/// literal or an unsupported escape.
fn read_string_literal(chars: &mut Peekable<Chars<'_>>) -> Option<String> {
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                d @ '0'..='7' => {
                    let mut code = d.to_digit(8)?;
                    for _ in 0..2 {
                        match chars.peek() {
                            Some(c @ '0'..='7') => {
                                code = code * 8 + c.to_digit(8)?;
                                chars.next();
                            }
                            _ => break,
                        }
                    }
                    out.push(char::from_u32(code)?);
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
}

/// Splits `s` (trimmed, starting with `(` and ending with `)`) into the
/// head symbol and the remainder after it, or `None` if the shape is wrong.
fn split_head<'a>(s: &'a str, head: &str) -> Option<&'a str> {
    let inner = s.trim().strip_prefix('(')?.strip_suffix(')')?.trim();
    let rest = inner.strip_prefix(head)?;
    // After the keyword there must be whitespace (or end of expression)
    if !rest.is_empty() && !rest.starts_with(|c: char| c.is_ascii_whitespace()) {
        return None;
    }
    Some(rest.trim_start())
}

/// Parses zero or more whitespace-separated string literals until the end of
/// `rest`.  Returns `None` on any other token.
fn parse_string_list(rest: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            chars.next();
        }
        match chars.peek() {
            None => break,
            Some('"') => {
                chars.next(); // consume opening `"`
                words.push(read_string_literal(&mut chars)?);
            }
            Some(_) => return None, // unexpected token
        }
    }
    Some(words)
}

/// Attempts to parse `(skk-ignore-dic-word "w1" "w2" ...)` and returns the
/// list of words to ignore, or `None` if the string does not match.
fn parse_ignore_dic_word(s: &str) -> Option<Vec<String>> {
    parse_string_list(split_head(s, "skk-ignore-dic-word")?)
}

/// Evaluates `(concat "s1" "s2" ...)` to the concatenated string.
///
/// Returns `None` if `s` is not a `concat` form made only of string literals
/// (such candidates stay `LispForm::Unknown`).
pub fn parse_concat(s: &str) -> Option<String> {
    Some(parse_string_list(split_head(s, "concat")?)?.concat())
}

/// Returns the byte offset just past the `)` that closes the S-expression
/// starting at the beginning of `s`, honouring string literals (a `)` or
/// `"` inside a literal does not count).  Returns `None` if `s` does not
/// start with `(` or the parentheses are unbalanced.
pub fn sexp_end(s: &str) -> Option<usize> {
    if !s.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Returns `true` if `s` cannot be written into a dictionary file verbatim
/// and must be wrapped with `quote_concat` instead: it contains a character
/// the dictionary syntax reserves (`/`, `;`, `"`, `\`, newline), or it starts
/// with `(` and would otherwise be read back as a Lisp form.
pub fn needs_quoting(s: &str) -> bool {
    s.starts_with('(') || s.contains(['/', ';', '"', '\\', '\n', '\r'])
}

/// Appends `s` to `out` as a double-quoted Lisp string literal, escaping it
/// the way DDSKK's `skk-quote-char` does so other SKK implementations read
/// it back correctly: `/` → `\057`, `;` → `\073`, `"` → `\"`, `\` → `\\`,
/// LF → `\n`, CR → `\r`.  This is the inverse of `read_string_literal` for
/// every character that would otherwise break the dictionary line format.
fn write_string_literal(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '/' => out.push_str("\\057"),
            ';' => out.push_str("\\073"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Wraps `s` as `(concat "...")` using `write_string_literal` escaping.
pub fn quote_concat(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 12);
    out.push_str("(concat ");
    write_string_literal(&mut out, s);
    out.push(')');
    out
}

/// Quotes `s` with `quote_concat` only when `needs_quoting` says so.
pub fn quote_if_needed(s: &str) -> String {
    if needs_quoting(s) {
        quote_concat(s)
    } else {
        s.to_string()
    }
}

/// Renders a `LispForm::IgnoreDicWord` back to its S-expression string.
/// Used when re-serializing a user dictionary that contains such an entry.
pub fn render_ignore_dic_word(words: &[String]) -> String {
    let mut out = String::from("(skk-ignore-dic-word");
    for w in words {
        out.push(' ');
        write_string_literal(&mut out, w);
    }
    out.push(')');
    out
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

    #[test]
    fn test_render_ignore_dic_word_escapes_reserved_characters() {
        // Octal and control escapes decoded on load must be re-encoded on
        // save; otherwise a raw '/' or newline would corrupt the dictionary.
        let words = vec!["and/or".to_string(), "a;b".to_string(), "x\ny".to_string()];
        let rendered = render_ignore_dic_word(&words);
        assert_eq!(
            rendered,
            "(skk-ignore-dic-word \"and\\057or\" \"a\\073b\" \"x\\ny\")"
        );
        assert!(!rendered.contains(['/', ';', '\n']));
        assert_eq!(classify(&rendered), Some(LispForm::IgnoreDicWord(words)));
    }

    // ── concat ───────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_concat_octal_escapes() {
        assert_eq!(
            parse_concat(r#"(concat "and\057or")"#).as_deref(),
            Some("and/or")
        );
        assert_eq!(
            parse_concat(r#"(concat "(\073_\073)")"#).as_deref(),
            Some("(;_;)")
        );
        assert_eq!(
            parse_concat(r#"(concat "http:\057\057example.com\057")"#).as_deref(),
            Some("http://example.com/")
        );
    }

    #[test]
    fn test_parse_concat_multiple_literals_and_escapes() {
        assert_eq!(
            parse_concat(r#"(concat "a" "b\"c" "\\d" "e\nf")"#).as_deref(),
            Some("ab\"c\\de\nf")
        );
        assert_eq!(parse_concat("(concat)").as_deref(), Some(""));
        assert_eq!(parse_concat("( concat \"x\" )").as_deref(), Some("x"));
    }

    #[test]
    fn test_parse_concat_rejects_non_literal_forms() {
        assert!(parse_concat("(concat foo)").is_none());
        assert!(parse_concat("(concat \"unterminated)").is_none());
        assert!(parse_concat("(concat \"bad\\q\")").is_none());
        assert!(parse_concat("(concatenate \"x\")").is_none());
        assert!(parse_concat("(skk-ignore-dic-word \"x\")").is_none());
        assert!(parse_concat("plain").is_none());
    }

    #[test]
    fn test_quote_concat_roundtrip() {
        for s in [
            "a/b",
            "x;y",
            "(;_;)",
            "say \"hi\"",
            "back\\slash",
            "line\nbreak",
            "(笑)",
        ] {
            let quoted = quote_concat(s);
            assert!(quoted.starts_with("(concat \""), "{quoted}");
            assert!(
                !quoted[9..quoted.len() - 2].contains(['/', ';']),
                "{quoted}"
            );
            assert_eq!(parse_concat(&quoted).as_deref(), Some(s), "{quoted}");
        }
        assert_eq!(quote_concat("a/b;c"), r#"(concat "a\057b\073c")"#);
    }

    #[test]
    fn test_needs_quoting() {
        assert!(!needs_quoting("漢字"));
        assert!(!needs_quoting("and or"));
        assert!(needs_quoting("and/or"));
        assert!(needs_quoting("a;b"));
        assert!(needs_quoting("(笑)"));
        assert!(needs_quoting("q\"q"));
        assert!(needs_quoting("a\\b"));
        assert!(needs_quoting("a\nb"));
        assert_eq!(quote_if_needed("漢字"), "漢字");
        assert_eq!(quote_if_needed("(笑)"), r#"(concat "(笑)")"#);
    }

    #[test]
    fn test_sexp_end() {
        assert_eq!(sexp_end("(concat \"a\")"), Some(12));
        assert_eq!(sexp_end("(concat \"a\");note"), Some(12));
        // `)` and `"` inside a string literal are ignored.
        let s = r#"(concat "x)\"y");ann"#;
        assert_eq!(sexp_end(s), Some(s.len() - 4));
        assert_eq!(sexp_end("(a (b c) d)"), Some(11));
        assert!(sexp_end("(unbalanced").is_none());
        assert!(sexp_end("plain").is_none());
    }
}
