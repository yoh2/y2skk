use super::entry::{DictEntry, DictError};

/// Synchronous dictionary provider.
/// Implemented by all dictionary backends (file-based, user dict, etc.).
pub trait DictionaryProvider: Send + Sync {
    /// Looks up a headword with optional okurigana.
    /// Returns `None` if not found.
    fn lookup(&self, midashi: &str, okuri: Option<&str>) -> Option<DictEntry>;

    /// Records a confirmed conversion into the dictionary (learning).
    fn learn(&mut self, entry: DictEntry) -> Result<(), DictError>;

    /// Priority used when multiple providers are chained (higher = searched first).
    fn priority(&self) -> i32 {
        0
    }
}

/// Asynchronous dictionary provider (for future script-based dictionaries).
pub trait AsyncDictionaryProvider: Send + Sync {
    fn lookup_async(
        &self,
        midashi: &str,
        okuri: Option<&str>,
    ) -> impl std::future::Future<Output = Option<DictEntry>> + Send;
}

/// Extension point for script-driven dictionary plugins (e.g. QuickJS).
/// Not implemented in v1; the trait is defined here so the architecture is in place.
pub trait ScriptDictPlugin: AsyncDictionaryProvider {
    fn engine_name(&self) -> &'static str;
    fn load_script(&mut self, source: &str) -> Result<(), Box<dyn std::error::Error>>;
}
