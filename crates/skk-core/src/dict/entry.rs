use thiserror::Error;

/// A dictionary entry: one headword with its list of candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictEntry {
    /// Headword (midashi)
    pub midashi: String,
    /// Okurigana, if present
    pub okuri: Option<String>,
    /// Candidate list (highest priority first)
    pub candidates: Vec<Candidate>,
}

/// A single conversion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The converted text
    pub word: String,
    /// Optional annotation (text after "; " in the dictionary)
    pub annotation: Option<String>,
}

impl Candidate {
    pub fn new(word: impl Into<String>) -> Self {
        Self { word: word.into(), annotation: None }
    }

    pub fn with_annotation(word: impl Into<String>, annotation: impl Into<String>) -> Self {
        Self { word: word.into(), annotation: Some(annotation.into()) }
    }
}

#[derive(Debug, Error)]
pub enum DictError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding error: {0}")]
    Encoding(String),
    #[error("parse error at line {line}: {message}")]
    Parse { line: usize, message: String },
    #[error("dictionary is read-only")]
    ReadOnly,
}
