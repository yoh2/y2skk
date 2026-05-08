pub mod parser;
pub mod table;

pub use table::{
    hiragana_to_halfwidth, hiragana_to_katakana, KanaMode, KanaTable, KanaTransition,
    TransitionResult,
};
