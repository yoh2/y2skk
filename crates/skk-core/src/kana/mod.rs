pub mod table;
pub mod parser;
pub mod builtin;

pub use table::{KanaTable, KanaTransition, KanaMode, KanaLayout, TransitionResult, hiragana_to_katakana, hiragana_to_halfwidth};
