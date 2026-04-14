pub mod table;
pub mod parser;

pub use table::{KanaTable, KanaTransition, KanaMode, TransitionResult, hiragana_to_katakana, hiragana_to_halfwidth};
