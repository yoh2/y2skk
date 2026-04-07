pub mod traits;
pub mod file;
pub mod entry;

pub use traits::{DictionaryProvider, AsyncDictionaryProvider};
pub use entry::{DictEntry, Candidate, DictError};
pub use file::{FileDict, UserDict, DictEncoding};
