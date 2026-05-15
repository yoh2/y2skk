/// Numeric conversion type, corresponding to the `#n` or `#x` marker in SKK
/// dictionary candidates.
///
/// Standard DDSKK types use digit suffixes (#0–#5, #9).
/// y2skk-specific extensions use letter suffixes (#6–#c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumType {
    /// `#0` — Raw ASCII digits as-is (e.g. "1234").
    Raw,
    /// `#1` — Full-width digits (e.g. "１２３４").
    Zenkaku,
    /// `#2` — Kanji positional notation (e.g. "一二三四").
    KanjiPos,
    /// `#3` — Kanji with place names (e.g. "千二百三十四").
    KanjiSeq,
    /// `#5` — Daiji (formal kanji for financial use, e.g. "壱阡弐百参拾四").
    Daiji,
    /// `#9` — Shogi notation (first digit full-width, rest kanji).
    Shogi,
    /// `#6` — Roman numerals using Unicode dedicated characters, uppercase
    ///         (e.g. "ⅯⅭⅭⅩⅩⅩⅣ").
    RomanUpperUnicode,
    /// `#7` — Roman numerals using Unicode dedicated characters, lowercase
    ///         (e.g. "ⅿⅽⅽⅹⅹⅹⅳ").
    RomanLowerUnicode,
    /// `#a` — Roman numerals using ASCII letters, uppercase (e.g. "MCCXXXIV").
    RomanUpperAscii,
    /// `#b` — Roman numerals using ASCII letters, lowercase (e.g. "mccxxxiv").
    RomanLowerAscii,
    /// `#c` — Circled numerals (e.g. "①", "⑳").  Only representable for 1–50.
    Circled,
}

impl NumType {
    /// Converts the marker character immediately following `#` in a dict entry
    /// to the corresponding `NumType`.  Returns `None` for unknown markers and
    /// for `#4`, which is recursive numeric conversion (handled in
    /// `crate::num::template::expand_with_recursive_lookup` via a separate
    /// branch that performs a dictionary re-lookup with the digit run as the
    /// secondary key).
    pub fn from_marker(ch: char) -> Option<Self> {
        match ch {
            '0' => Some(Self::Raw),
            '1' => Some(Self::Zenkaku),
            '2' => Some(Self::KanjiPos),
            '3' => Some(Self::KanjiSeq),
            '5' => Some(Self::Daiji),
            '9' => Some(Self::Shogi),
            '6' => Some(Self::RomanUpperUnicode),
            '7' => Some(Self::RomanLowerUnicode),
            'a' => Some(Self::RomanUpperAscii),
            'b' => Some(Self::RomanLowerAscii),
            'c' => Some(Self::Circled),
            _ => None,
        }
    }

    /// Returns the marker character for this type.
    pub fn marker(self) -> char {
        match self {
            Self::Raw => '0',
            Self::Zenkaku => '1',
            Self::KanjiPos => '2',
            Self::KanjiSeq => '3',
            Self::Daiji => '5',
            Self::Shogi => '9',
            Self::RomanUpperUnicode => '6',
            Self::RomanLowerUnicode => '7',
            Self::RomanUpperAscii => 'a',
            Self::RomanLowerAscii => 'b',
            Self::Circled => 'c',
        }
    }

    /// All types used for synthesizing candidates when the midashi contains a
    /// digit run.  Standard types come first for natural ordering.
    pub const SYNTH_TYPES: &'static [NumType] = &[
        Self::Raw,
        Self::Zenkaku,
        Self::KanjiPos,
        Self::KanjiSeq,
        Self::Daiji,
        Self::Shogi,
        Self::RomanUpperUnicode,
        Self::RomanLowerUnicode,
        Self::RomanUpperAscii,
        Self::RomanLowerAscii,
        Self::Circled,
    ];
}
