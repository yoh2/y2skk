use super::{convert, types::NumType};

/// Scans `midashi` for maximal runs of ASCII decimal digits, replaces each run
/// with a single `#`, and returns the resulting template string together with
/// the ordered list of digit runs.
///
/// # Examples
/// ```
/// # use skk_core::num::template::scan;
/// let (tmpl, runs) = scan("だい12かい");
/// assert_eq!(tmpl, "だい#かい");
/// assert_eq!(runs, vec!["12"]);
///
/// let (tmpl, runs) = scan("1234");
/// assert_eq!(tmpl, "#");
/// assert_eq!(runs, vec!["1234"]);
///
/// let (tmpl, runs) = scan("abc");
/// assert_eq!(tmpl, "abc");
/// assert!(runs.is_empty());
/// ```
pub fn scan(midashi: &str) -> (String, Vec<String>) {
    let mut template = String::with_capacity(midashi.len());
    let mut runs: Vec<String> = Vec::new();
    let mut in_run = false;
    let mut run_buf = String::new();

    for ch in midashi.chars() {
        if ch.is_ascii_digit() {
            in_run = true;
            run_buf.push(ch);
        } else {
            if in_run {
                runs.push(run_buf.clone());
                run_buf.clear();
                template.push('#');
                in_run = false;
            }
            template.push(ch);
        }
    }
    if in_run {
        runs.push(run_buf);
        template.push('#');
    }
    (template, runs)
}

/// Expands `#n` markers in `candidate_word` using the digit runs extracted by
/// [`scan`].
///
/// Each `#X` marker (where X is a known [`NumType`] marker character) is
/// replaced by the Nth occurrence of a digit run converted with the type for X.
/// Multiple markers independently index into `runs` by their sequential
/// appearance in the candidate string.
///
/// Returns `None` if:
/// - The candidate references more `#X` markers than there are runs.
/// - A conversion for a specific run+type combination returns `None` (i.e. the
///   value is not representable by that type).
pub fn expand(candidate_word: &str, runs: &[String]) -> Option<String> {
    let mut result = String::new();
    let mut chars = candidate_word.chars().peekable();
    let mut run_index = 0usize;

    while let Some(ch) = chars.next() {
        if ch == '#' {
            match chars.peek().copied().and_then(NumType::from_marker) {
                Some(ty) => {
                    // Consume the marker character.
                    chars.next();
                    let digits = runs.get(run_index)?;
                    run_index += 1;
                    let converted = convert(digits, ty)?;
                    result.push_str(&converted);
                }
                None => {
                    // Unknown or unsupported marker (includes `#4`): keep as-is.
                    result.push('#');
                }
            }
        } else {
            result.push(ch);
        }
    }
    Some(result)
}

/// Generates synthetic candidates by converting the digit runs with every type
/// in `types`, concatenating the per-run results.
///
/// For a midashi with a single digit run `["1234"]`, each type produces one
/// candidate string.  For multiple runs, the results for each run are
/// concatenated in order.  If any run cannot be converted for a given type,
/// that type is skipped entirely.
///
/// The returned `Vec` has one entry per successfully converted type, in the
/// same order as `types`.
pub fn synthesize(runs: &[String], types: &[NumType]) -> Vec<String> {
    if runs.is_empty() {
        return Vec::new();
    }
    types
        .iter()
        .filter_map(|&ty| {
            let mut combined = String::new();
            for run in runs {
                let part = convert(run, ty)?;
                combined.push_str(&part);
            }
            Some(combined)
        })
        .collect()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_single_run() {
        let (tmpl, runs) = scan("だい12かい");
        assert_eq!(tmpl, "だい#かい");
        assert_eq!(runs, vec!["12".to_string()]);
    }

    #[test]
    fn scan_only_digits() {
        let (tmpl, runs) = scan("1234");
        assert_eq!(tmpl, "#");
        assert_eq!(runs, vec!["1234".to_string()]);
    }

    #[test]
    fn scan_no_digits() {
        let (tmpl, runs) = scan("あいうえお");
        assert_eq!(tmpl, "あいうえお");
        assert!(runs.is_empty());
    }

    #[test]
    fn scan_multiple_runs() {
        let (tmpl, runs) = scan("2がつ25にち");
        assert_eq!(tmpl, "#がつ#にち");
        assert_eq!(runs, vec!["2".to_string(), "25".to_string()]);
    }

    #[test]
    fn scan_leading_trailing_digits() {
        let (tmpl, runs) = scan("123abc456");
        assert_eq!(tmpl, "#abc#");
        assert_eq!(runs, vec!["123".to_string(), "456".to_string()]);
    }

    #[test]
    fn expand_single_type() {
        let (_, runs) = scan("だい12かい");
        assert_eq!(expand("第#1回", &runs), Some("第１２回".to_string()));
        assert_eq!(expand("第#0回", &runs), Some("第12回".to_string()));
        assert_eq!(expand("第#3回", &runs), Some("第十二回".to_string()));
    }

    #[test]
    fn expand_multiple_runs() {
        let (_, runs) = scan("2がつ25にち");
        assert_eq!(expand("#0年#2月", &runs), Some("2年二五月".to_string()));
        assert_eq!(expand("#1月#1日", &runs), Some("２月２５日".to_string()));
    }

    #[test]
    fn expand_insufficient_runs() {
        let (_, runs) = scan("12");
        // Two #markers but only one run → None
        assert_eq!(expand("#0年#1月", &runs), None);
    }

    #[test]
    fn expand_unknown_marker_kept() {
        let (_, runs) = scan("12");
        // #4 is unsupported, kept as-is; #0 uses the run.
        let result = expand("#4#0", &runs);
        // #4 keeps '#' then '4', then #0 → "12" — wait, #4 peeks '4' which is
        // NOT in from_marker (we excluded it), so '#' is kept literal and '4' is
        // consumed by the next iteration as a regular char.
        // Result: "#4" + "12" = "#412"  (run_index still advances to 1 for #0).
        assert_eq!(result, Some("#412".to_string()));
    }

    #[test]
    fn synthesize_single_run() {
        let (_, runs) = scan("12");
        let candidates = synthesize(&runs, &[NumType::Raw, NumType::Zenkaku, NumType::KanjiSeq]);
        assert_eq!(
            candidates,
            vec!["12".to_string(), "１２".to_string(), "十二".to_string()]
        );
    }

    #[test]
    fn synthesize_unrepresentable_skipped() {
        let (_, runs) = scan("1234");
        let candidates = synthesize(&runs, &[NumType::Circled, NumType::Raw]);
        // Circled is skipped for 1234 (> 50); Raw succeeds.
        assert_eq!(candidates, vec!["1234".to_string()]);
    }

    #[test]
    fn synthesize_multiple_runs_concatenated() {
        let (_, runs) = scan("2がつ25にち");
        let candidates = synthesize(&runs, &[NumType::Raw, NumType::Zenkaku]);
        // Raw: "2" + "25" = "225"; Zenkaku: "２" + "２５" = "２２５"
        assert_eq!(candidates, vec!["225".to_string(), "２２５".to_string()]);
    }
}
