pub mod convert;
pub mod template;
pub mod types;

pub use convert::convert;
pub use template::{expand, expand_with_recursive_lookup, scan, synthesize};
pub use types::NumType;

/// Returns true if `midashi` contains at least one ASCII decimal digit run.
pub fn has_digit_run(midashi: &str) -> bool {
    midashi.chars().any(|c| c.is_ascii_digit())
}
