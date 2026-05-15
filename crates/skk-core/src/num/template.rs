use super::{convert, types::NumType};

/// Defensive cap on the number of expansions a single dict candidate may
/// produce via [`expand_with_recursive_lookup`]. Templates containing
/// multiple `#4` markers can in principle produce a cartesian-product blow-up
/// (`alternatives_1 * alternatives_2 * ...`); this bound keeps memory and
/// time predictable for pathological or crafted dictionaries.
pub const MAX_RECURSIVE_EXPANSIONS: usize = 256;

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

/// Expands `#n` markers in `candidate_word` using digit runs from [`scan`],
/// with `#4` (recursive numeric conversion) handled by a `lookup` callback.
///
/// Each `#4` marker consumes the next run from `runs` and is replaced by every
/// result of `lookup(run)`. Multiple `#4` markers combine via cartesian
/// product, so `"#4-#4"` with two runs and lookup results `[A, B]` and `[C]`
/// produces `["A-C", "B-C"]`. Other `#X` markers are resolved by
/// [`crate::num::convert::convert`] just like in [`expand`].
///
/// Returns an empty `Vec` only if a *recognised* marker fails to resolve —
/// specifically: run shortage (more `#n` markers than runs), `convert`
/// failure for a `#0`/`#1`/`#2`/`#3`/`#5`/`#9`/`#6`/`#7`/`#a`/`#b`/`#c`
/// marker, or a `#4` whose recursive lookup returns no candidates.
/// Unknown or unsupported markers (e.g. `#z`) are *not* failures: the
/// `#` is kept as a literal character and processing continues, matching
/// [`expand`]. Recursive lookup results are substituted literally — they
/// are not re-scanned for nested markers, so a dictionary chain such as
/// `5 /#4/` produces the literal string `"#4"` rather than recursing
/// indefinitely.
///
/// For candidates that do not contain `#4`, this behaves like `expand`
/// wrapped in a single-element `Vec`, and `lookup` is never invoked.
///
/// Defensive cap: a template containing multiple `#4` markers can in
/// principle produce `lookup_count_1 * lookup_count_2 * ...` expansions.
/// To prevent pathological dictionaries (or crafted inputs) from blowing
/// up memory, the running expansion set is capped at
/// [`MAX_RECURSIVE_EXPANSIONS`] entries; any additional combinations are
/// silently dropped.
pub fn expand_with_recursive_lookup(
    candidate_word: &str,
    runs: &[String],
    lookup: &dyn Fn(&str) -> Vec<String>,
) -> Vec<String> {
    let mut results: Vec<String> = vec![String::new()];
    let mut chars = candidate_word.chars().peekable();
    let mut run_index = 0usize;

    while let Some(ch) = chars.next() {
        if ch == '#' {
            let next = chars.peek().copied();
            if next == Some('4') {
                chars.next(); // consume '4'
                let Some(run) = runs.get(run_index) else {
                    return Vec::new();
                };
                run_index += 1;
                let alternatives = lookup(run);
                if alternatives.is_empty() {
                    return Vec::new();
                }
                let projected = results.len().saturating_mul(alternatives.len());
                let cap = projected.min(MAX_RECURSIVE_EXPANSIONS);
                let mut new_results = Vec::with_capacity(cap);
                'outer: for r in &results {
                    for alt in &alternatives {
                        if new_results.len() >= MAX_RECURSIVE_EXPANSIONS {
                            break 'outer;
                        }
                        let mut s = r.clone();
                        s.push_str(alt);
                        new_results.push(s);
                    }
                }
                results = new_results;
            } else if let Some(ty) = next.and_then(NumType::from_marker) {
                chars.next(); // consume marker
                let Some(digits) = runs.get(run_index) else {
                    return Vec::new();
                };
                run_index += 1;
                let Some(converted) = convert(digits, ty) else {
                    return Vec::new();
                };
                for r in &mut results {
                    r.push_str(&converted);
                }
            } else {
                // Unknown marker: keep '#' literal (matches `expand` behavior).
                for r in &mut results {
                    r.push('#');
                }
            }
        } else {
            for r in &mut results {
                r.push(ch);
            }
        }
    }
    results
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

    // ── #4 (recursive numeric conversion) tests ──────────────────────────────

    #[test]
    fn expand_recursive_basic() {
        // DDSKK manual example: dict has "p# /#4/" and "125 /東京都葛飾区/"
        let lookup = |key: &str| -> Vec<String> {
            if key == "125" {
                vec!["東京都葛飾区".to_string()]
            } else {
                vec![]
            }
        };
        let runs = vec!["125".to_string()];
        let result = expand_with_recursive_lookup("#4", &runs, &lookup);
        assert_eq!(result, vec!["東京都葛飾区".to_string()]);
    }

    #[test]
    fn expand_recursive_multiple_candidates() {
        // One #4 with a recursive lookup that returns several candidates.
        let lookup = |key: &str| -> Vec<String> {
            if key == "1" {
                vec!["A".to_string(), "B".to_string(), "C".to_string()]
            } else {
                vec![]
            }
        };
        let runs = vec!["1".to_string()];
        let result = expand_with_recursive_lookup("#4", &runs, &lookup);
        assert_eq!(
            result,
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
    }

    #[test]
    fn expand_recursive_cartesian() {
        // Two #4 markers fan out via cartesian product.
        let lookup = |key: &str| -> Vec<String> {
            match key {
                "1" => vec!["A".to_string(), "B".to_string()],
                "2" => vec!["C".to_string()],
                _ => vec![],
            }
        };
        let runs = vec!["1".to_string(), "2".to_string()];
        let result = expand_with_recursive_lookup("#4-#4", &runs, &lookup);
        assert_eq!(result, vec!["A-C".to_string(), "B-C".to_string()]);
    }

    #[test]
    fn expand_recursive_mixed_with_other_markers() {
        // #0 and #4 mixed in one template.
        let lookup = |key: &str| -> Vec<String> {
            if key == "2" {
                vec!["X".to_string()]
            } else {
                vec![]
            }
        };
        let runs = vec!["1".to_string(), "2".to_string()];
        let result = expand_with_recursive_lookup("#0/#4", &runs, &lookup);
        assert_eq!(result, vec!["1/X".to_string()]);
    }

    #[test]
    fn expand_recursive_lookup_miss_returns_empty() {
        // Recursive lookup returning no candidates fails the whole expansion.
        let lookup = |_: &str| -> Vec<String> { vec![] };
        let runs = vec!["999".to_string()];
        let result = expand_with_recursive_lookup("#4", &runs, &lookup);
        assert!(result.is_empty());
    }

    #[test]
    fn expand_recursive_self_reference_substituted_literally() {
        // If the recursive lookup result itself contains a #4 marker, it is
        // substituted as a literal string — no second pass of expansion.
        let lookup = |key: &str| -> Vec<String> {
            if key == "5" {
                vec!["#4".to_string()]
            } else {
                vec![]
            }
        };
        let runs = vec!["5".to_string()];
        let result = expand_with_recursive_lookup("#4", &runs, &lookup);
        assert_eq!(result, vec!["#4".to_string()]);
    }

    #[test]
    fn expand_recursive_no_recursive_marker_compatible_with_expand() {
        // Templates without #4 behave just like `expand` wrapped in a Vec.
        // The lookup callback must not be invoked.
        let lookup = |_: &str| -> Vec<String> {
            panic!("lookup must not be called when no #4 is present");
        };
        let runs = vec!["3".to_string()];
        let result = expand_with_recursive_lookup("#1月", &runs, &lookup);
        assert_eq!(result, vec!["３月".to_string()]);
    }

    #[test]
    fn expand_recursive_run_shortage_returns_empty() {
        // Template references more #4 than there are runs.
        let lookup = |_: &str| -> Vec<String> { vec!["X".to_string()] };
        let runs: Vec<String> = vec![]; // no runs
        let result = expand_with_recursive_lookup("#4", &runs, &lookup);
        assert!(result.is_empty());
    }

    #[test]
    fn expand_recursive_caps_cartesian_product() {
        // Multiple #4 markers each with many lookup results would otherwise
        // produce alternatives_1 * alternatives_2 * ... expansions
        // (here 16^4 = 65536); the cap keeps the output bounded.
        let alternatives: Vec<String> = (0..16).map(|i| format!("a{i}")).collect();
        let lookup = |_: &str| alternatives.clone();
        let runs: Vec<String> = (0..4).map(|i| i.to_string()).collect();
        let result = expand_with_recursive_lookup("#4#4#4#4", &runs, &lookup);
        assert!(
            result.len() <= MAX_RECURSIVE_EXPANSIONS,
            "expansion must be capped at {}, got {}",
            MAX_RECURSIVE_EXPANSIONS,
            result.len()
        );
        assert_eq!(
            result.len(),
            MAX_RECURSIVE_EXPANSIONS,
            "with 16^4 potential expansions the cap should be saturated"
        );
        // The first emitted combinations must be well-formed concatenations
        // of four alternatives (no truncation mid-string).
        for s in &result {
            assert!(s.starts_with('a'));
            assert!(!s.contains('#'));
        }
    }
}
