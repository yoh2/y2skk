/// SKK keybinding configuration.
///
/// In v1, defaults are hardcoded. In a future version, these can be overridden
/// via `[keybindings]` in `config.toml`.
#[derive(Debug, Clone)]
pub struct SkkKeybindings {
    /// Characters that switch to ASCII (latin) mode (default: `['l']`)
    pub ascii_mode: Vec<char>,
    /// Characters that switch to wide-ASCII mode (default: `['L']`)
    pub wide_ascii_mode: Vec<char>,
    /// Characters that switch to katakana mode (default: `['q']`)
    pub katakana_mode: Vec<char>,
    /// Characters that start abbrev (romaji search) mode (default: `['/']`)
    pub abbrev_mode: Vec<char>,
    /// Characters that cancel / shrink the current operation (default: `['x']`)
    pub cancel: Vec<char>,
}

impl Default for SkkKeybindings {
    fn default() -> Self {
        Self {
            ascii_mode: vec!['l'],
            wide_ascii_mode: vec!['L'],
            katakana_mode: vec!['q'],
            abbrev_mode: vec!['/'],
            cancel: vec!['x'],
        }
    }
}
