use super::types::NumType;

/// Converts an ASCII decimal digit string using the given `NumType`.
/// Returns `None` when the conversion is not representable for the given type
/// (e.g. circled numerals outside 1–50, Roman numerals for 0 or values > 3999).
pub fn convert(digits: &str, ty: NumType) -> Option<String> {
    match ty {
        NumType::Raw => Some(digits.to_string()),
        NumType::Zenkaku => Some(to_zenkaku(digits)),
        NumType::KanjiPos => Some(to_kanji_positional(digits)),
        NumType::KanjiSeq => to_kanji_sequential(digits),
        NumType::Daiji => to_daiji(digits),
        NumType::Shogi => to_shogi(digits),
        NumType::RomanUpperUnicode => to_roman_unicode(parse_u64(digits)?, true),
        NumType::RomanLowerUnicode => to_roman_unicode(parse_u64(digits)?, false),
        NumType::RomanUpperAscii => to_roman_ascii(parse_u64(digits)?, true),
        NumType::RomanLowerAscii => to_roman_ascii(parse_u64(digits)?, false),
        NumType::Circled => to_circled(parse_u64(digits)?),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn parse_u64(digits: &str) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// #1 — each digit to its full-width Unicode counterpart.
fn to_zenkaku(digits: &str) -> String {
    digits
        .chars()
        .map(|c| match c {
            '0' => '０',
            '1' => '１',
            '2' => '２',
            '3' => '３',
            '4' => '４',
            '5' => '５',
            '6' => '６',
            '7' => '７',
            '8' => '８',
            '9' => '９',
            other => other,
        })
        .collect()
}

/// #2 — positional kanji: each digit mapped to its kanji character.
/// e.g. "5500" → "五五〇〇"
fn to_kanji_positional(digits: &str) -> String {
    digits
        .chars()
        .map(|c| match c {
            '0' => '〇',
            '1' => '一',
            '2' => '二',
            '3' => '三',
            '4' => '四',
            '5' => '五',
            '6' => '六',
            '7' => '七',
            '8' => '八',
            '9' => '九',
            other => other,
        })
        .collect()
}

/// #3 — sequential kanji with place values (十 百 千 万 億 兆 ...).
/// e.g. "5500" → "五千五百", "10" → "十", "100" → "百"
fn to_kanji_sequential(digits: &str) -> Option<String> {
    let n: u64 = parse_u64(digits)?;
    Some(kanji_number(n, KANJI_DIGITS, KANJI_SMALL_UNITS, true))
}

/// #5 — daiji (formal kanji used in financial documents).
/// e.g. "1995" → "壱阡九百九拾伍"
fn to_daiji(digits: &str) -> Option<String> {
    let n: u64 = parse_u64(digits)?;
    Some(kanji_number(n, DAIJI_DIGITS, DAIJI_SMALL_UNITS, false))
}

const KANJI_DIGITS: &[char] = &['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
const DAIJI_DIGITS: &[char] = &['零', '壱', '弐', '参', '四', '伍', '六', '七', '八', '九'];

/// Large-unit separators (万, 億, 兆, 京).  Each value is exactly 10^4, 10^8, 10^12, 10^16.
const LARGE_UNITS: &[(u64, &str)] = &[
    (10_000_000_000_000_000, "京"), // 10^16
    (1_000_000_000_000, "兆"),      // 10^12
    (100_000_000, "億"),            // 10^8
    (10_000, "万"),                 // 10^4
];

/// Small-unit separators within a group of up to 9999 for standard kanji.
const KANJI_SMALL_UNITS: &[(u64, &str)] = &[(1_000, "千"), (100, "百"), (10, "十")];
/// Small-unit separators within a group of up to 9999 for daiji.
const DAIJI_SMALL_UNITS: &[(u64, &str)] = &[(1_000, "阡"), (100, "百"), (10, "拾")];

/// Builds a kanji/daiji numeral string.
///
/// Numbers are decomposed into groups of up to 9999 separated by 万, 億, 兆, 京.
/// Within each group, `small_units` provides the place-name strings (千/阡, 百, 十/拾).
/// When `omit_leading_one` is true, a coefficient of 1 before a small-unit place name
/// is suppressed (standard kanji: 千 not 一千, but 一万 is kept because 万 is a large unit).
fn kanji_number(
    n: u64,
    digit_chars: &[char],
    small_units: &[(u64, &str)],
    omit_leading_one: bool,
) -> String {
    if n == 0 {
        return digit_chars[0].to_string();
    }
    let mut result = String::new();
    let mut rem = n;

    // Process large units (万 and above).
    for &(unit_val, unit_name) in LARGE_UNITS {
        let group = rem / unit_val;
        if group > 0 {
            // The coefficient before the large unit is always written out (no omission).
            result.push_str(&kanji_group(
                group,
                digit_chars,
                small_units,
                omit_leading_one,
            ));
            result.push_str(unit_name);
            rem -= group * unit_val;
        }
    }

    // Remaining value (< 10000).
    if rem > 0 {
        result.push_str(&kanji_group(
            rem,
            digit_chars,
            small_units,
            omit_leading_one,
        ));
    }
    result
}

/// Converts a number 1–9999 to its kanji representation within a group.
fn kanji_group(
    n: u64,
    digit_chars: &[char],
    small_units: &[(u64, &str)],
    omit_leading_one: bool,
) -> String {
    let mut result = String::new();
    let mut rem = n;
    for &(place_val, place_name) in small_units {
        let d = rem / place_val;
        if d > 0 {
            if d == 1 && omit_leading_one {
                // Standard kanji: omit "一" before 千/百/十.
            } else {
                result.push(digit_chars[d as usize]);
            }
            result.push_str(place_name);
            rem -= d * place_val;
        }
    }
    // Ones digit.
    if rem > 0 {
        result.push(digit_chars[rem as usize]);
    }
    result
}

/// #9 — shogi notation: first digit full-width, rest positional kanji.
/// e.g. "34" → "３四"
fn to_shogi(digits: &str) -> Option<String> {
    let mut chars = digits.chars();
    let first = chars.next()?;
    let first_fw = to_zenkaku(&first.to_string());
    let rest_kanji: String = chars
        .map(|c| match c {
            '1' => '一',
            '2' => '二',
            '3' => '三',
            '4' => '四',
            '5' => '五',
            '6' => '六',
            '7' => '七',
            '8' => '八',
            '9' => '九',
            other => other,
        })
        .collect();
    Some(format!("{}{}", first_fw, rest_kanji))
}

// ── Roman numerals ─────────────────────────────────────────────────────────

const ROMAN_VALUES: &[(u64, &str, &str)] = &[
    (1000, "M", "Ⅿ"),
    (900, "CM", "ⅭⅯ"),
    (500, "D", "Ⅾ"),
    (400, "CD", "ⅭⅮ"),
    (100, "C", "Ⅽ"),
    (90, "XC", "ⅩⅭ"),
    (50, "L", "Ⅼ"),
    (40, "XL", "ⅩⅬ"),
    (10, "X", "Ⅹ"),
    (9, "IX", "Ⅸ"),
    (5, "V", "Ⅴ"),
    (4, "IV", "Ⅳ"),
    (1, "I", "Ⅰ"),
];

/// Returns `None` for n == 0 or n > 3999 (not representable).
fn to_roman_unicode(n: u64, uppercase: bool) -> Option<String> {
    if n == 0 || n > 3999 {
        return None;
    }
    let mut result = String::new();
    let mut rem = n;
    for &(val, _ascii, unicode) in ROMAN_VALUES {
        while rem >= val {
            // Unicode Roman numeral block has both uppercase (U+2160+) and
            // lowercase (U+2170+) forms, offset by 0x10.
            let s: String = if uppercase {
                unicode.to_string()
            } else {
                unicode
                    .chars()
                    .map(|c| char::from_u32(c as u32 + 0x10).unwrap_or(c))
                    .collect()
            };
            result.push_str(&s);
            rem -= val;
        }
    }
    Some(result)
}

fn to_roman_ascii(n: u64, uppercase: bool) -> Option<String> {
    if n == 0 || n > 3999 {
        return None;
    }
    let mut result = String::new();
    let mut rem = n;
    for &(val, ascii, _unicode) in ROMAN_VALUES {
        while rem >= val {
            result.push_str(ascii);
            rem -= val;
        }
    }
    if uppercase {
        Some(result)
    } else {
        Some(result.to_lowercase())
    }
}

// ── Circled numerals ────────────────────────────────────────────────────────

/// Returns the circled numeral for 1–50.  Returns `None` outside that range.
fn to_circled(n: u64) -> Option<String> {
    if n == 0 || n > 50 {
        return None;
    }
    // Unicode: ① = U+2460 (1), ..., ⑳ = U+2473 (20)
    //          ㉑ = U+3251 (21), ..., ㉟ = U+325F (35)
    //          ㊱ = U+3260 — wait, let's use the correct ranges:
    //          ① U+2460 – ⑳ U+2473 : 1–20
    //          ㉑ U+3251 – ㉟ U+325F : 21–35
    //          ㊱ U+32B1 – ㊿ U+32BF : 36–50
    let cp: u32 = if n <= 20 {
        0x2460 + n as u32 - 1
    } else if n <= 35 {
        0x3251 + n as u32 - 21
    } else {
        0x32B1 + n as u32 - 36
    };
    char::from_u32(cp).map(|c| c.to_string())
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw() {
        assert_eq!(convert("1234", NumType::Raw), Some("1234".into()));
        assert_eq!(convert("0", NumType::Raw), Some("0".into()));
    }

    #[test]
    fn zenkaku() {
        assert_eq!(convert("1234", NumType::Zenkaku), Some("１２３４".into()));
        assert_eq!(convert("0", NumType::Zenkaku), Some("０".into()));
    }

    #[test]
    fn kanji_positional() {
        assert_eq!(convert("1234", NumType::KanjiPos), Some("一二三四".into()));
        assert_eq!(convert("5500", NumType::KanjiPos), Some("五五〇〇".into()));
        assert_eq!(convert("0", NumType::KanjiPos), Some("〇".into()));
    }

    #[test]
    fn kanji_sequential() {
        assert_eq!(
            convert("1234", NumType::KanjiSeq),
            Some("千二百三十四".into())
        );
        assert_eq!(convert("5500", NumType::KanjiSeq), Some("五千五百".into()));
        assert_eq!(convert("10", NumType::KanjiSeq), Some("十".into()));
        assert_eq!(convert("100", NumType::KanjiSeq), Some("百".into()));
        assert_eq!(convert("1000", NumType::KanjiSeq), Some("千".into()));
        assert_eq!(convert("10000", NumType::KanjiSeq), Some("一万".into()));
        assert_eq!(convert("12", NumType::KanjiSeq), Some("十二".into()));
        assert_eq!(convert("0", NumType::KanjiSeq), Some("〇".into()));
        assert_eq!(
            convert("100000000000000000", NumType::KanjiSeq),
            Some("十京".into())
        );
    }

    #[test]
    fn daiji() {
        assert_eq!(
            convert("1995", NumType::Daiji),
            Some("壱阡九百九拾伍".into())
        );
        assert_eq!(
            convert("1234", NumType::Daiji),
            Some("壱阡弐百参拾四".into())
        );
        assert_eq!(convert("0", NumType::Daiji), Some("零".into()));
    }

    #[test]
    fn shogi() {
        assert_eq!(convert("34", NumType::Shogi), Some("３四".into()));
        assert_eq!(convert("19", NumType::Shogi), Some("１九".into()));
        // Single digit
        assert_eq!(convert("5", NumType::Shogi), Some("５".into()));
    }

    #[test]
    fn roman_unicode() {
        assert_eq!(
            convert("1234", NumType::RomanUpperUnicode),
            Some("ⅯⅭⅭⅩⅩⅩⅣ".into())
        );
        assert_eq!(
            convert("1234", NumType::RomanLowerUnicode),
            Some("ⅿⅽⅽⅹⅹⅹⅳ".into())
        );
        assert_eq!(convert("4000", NumType::RomanUpperUnicode), None);
        assert_eq!(convert("0", NumType::RomanUpperUnicode), None);
    }

    #[test]
    fn roman_ascii() {
        assert_eq!(
            convert("1234", NumType::RomanUpperAscii),
            Some("MCCXXXIV".into())
        );
        assert_eq!(
            convert("1234", NumType::RomanLowerAscii),
            Some("mccxxxiv".into())
        );
        assert_eq!(
            convert("3999", NumType::RomanUpperAscii),
            Some("MMMCMXCIX".into())
        );
        assert_eq!(convert("4000", NumType::RomanUpperAscii), None);
    }

    #[test]
    fn circled() {
        assert_eq!(convert("1", NumType::Circled), Some("①".into()));
        assert_eq!(convert("20", NumType::Circled), Some("⑳".into()));
        assert_eq!(convert("21", NumType::Circled), Some("㉑".into()));
        assert_eq!(convert("35", NumType::Circled), Some("㉟".into()));
        assert_eq!(convert("36", NumType::Circled), Some("㊱".into()));
        assert_eq!(convert("50", NumType::Circled), Some("㊿".into()));
        assert_eq!(convert("51", NumType::Circled), None);
        assert_eq!(convert("0", NumType::Circled), None);
        assert_eq!(convert("1234", NumType::Circled), None);
    }
}
