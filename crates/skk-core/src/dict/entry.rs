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
    /// The converted text (or the raw S-expression string for Lisp-form candidates).
    pub word: String,
    /// Optional annotation (text after `;` in the dictionary).
    /// Ignored for Lisp-form candidates.
    pub annotation: Option<String>,
    /// If `Some`, this candidate is a Lisp-form entry (word starts with `(`).
    /// Lisp-form candidates are never displayed to the user; `word` holds the
    /// raw S-expression for round-trip serialization.
    pub lisp_form: Option<LispForm>,
}

/// Parsed form of a Lisp-form dictionary candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LispForm {
    /// `(skk-ignore-dic-word "w1" "w2" ...)` — words to exclude from candidates.
    IgnoreDicWord(Vec<String>),
    /// An S-expression that is not yet interpreted.
    /// Preserved as-is for round-trip serialization.
    Unknown,
}

impl Candidate {
    pub fn new(word: impl Into<String>) -> Self {
        Self {
            word: word.into(),
            annotation: None,
            lisp_form: None,
        }
    }

    pub fn with_annotation(word: impl Into<String>, annotation: impl Into<String>) -> Self {
        Self {
            word: word.into(),
            annotation: Some(annotation.into()),
            lisp_form: None,
        }
    }

    /// Creates a Lisp-form candidate.  `raw` is the full S-expression string;
    /// it is stored in `word` for round-trip serialization.
    pub fn lisp(raw: impl Into<String>, form: LispForm) -> Self {
        Self {
            word: raw.into(),
            annotation: None,
            lisp_form: Some(form),
        }
    }

    /// Returns `true` if this candidate should be displayed to the user.
    /// Lisp-form candidates (e.g. directives) are hidden.
    pub fn is_displayable(&self) -> bool {
        self.lisp_form.is_none()
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
