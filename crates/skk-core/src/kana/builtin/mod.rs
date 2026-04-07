use super::table::{KanaLayout, KanaTable};
use super::parser::parse_table;

const ROMAJI_TABLE: &str = include_str!("romaji.txt");
const AZIK_US_TABLE: &str = include_str!("azik-us.txt");
const AZIK_JP_TABLE: &str = include_str!("azik-jp.txt");
const DVORAKJP_US_TABLE: &str = include_str!("dvorakjp-us.txt");
const DVORAKJP_JP_TABLE: &str = include_str!("dvorakjp-jp.txt");

/// Returns the built-in kana conversion table for the given layout.
/// Panics if the embedded table data is malformed (indicates a bug, not a runtime error).
pub fn builtin_table(layout: KanaLayout) -> KanaTable {
    let src = match layout {
        KanaLayout::Romaji => ROMAJI_TABLE,
        KanaLayout::AzikUs => AZIK_US_TABLE,
        KanaLayout::AzikJp => AZIK_JP_TABLE,
        KanaLayout::DvorakJpUs => DVORAKJP_US_TABLE,
        KanaLayout::DvorakJpJp => DVORAKJP_JP_TABLE,
    };
    parse_table(src).unwrap_or_else(|e| {
        panic!("built-in kana table is invalid: {e}");
    })
}
