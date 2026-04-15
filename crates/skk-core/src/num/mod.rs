pub mod types;
pub mod convert;
pub mod template;

pub use types::NumType;
pub use convert::convert;
pub use template::{scan, expand, synthesize};

/// Returns true if `midashi` contains at least one ASCII decimal digit run.
pub fn has_digit_run(midashi: &str) -> bool {
    midashi.chars().any(|c| c.is_ascii_digit())
}
