use std::env;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};
use skk_core::dict::DictEncoding;

/// Expands a leading `~` component to the user's home directory.
/// Returns the path unchanged if it does not start with `~` or if the home
/// directory cannot be determined.
fn expand_tilde(path: PathBuf) -> PathBuf {
    let mut components = path.components();
    if let Some(Component::Normal(c)) = components.next() {
        if c == OsStr::new("~") {
            if let Some(home) = dirs::home_dir() {
                return home.join(components.as_path());
            }
        }
    }
    path
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("TOML parse error in {path}: {source}")]
    Toml {
        path: PathBuf,
        source: toml::de::Error,
    },
}

// ── Top-level config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub input: InputConfig,
    #[serde(rename = "user-dict")]
    pub user_dict: UserDictConfig,
    pub dict: DictConfig,
    pub indicator: IndicatorConfig,
    pub candidates: CandidatesConfig,
    pub daemon: DaemonConfig,
}

// ── [input] ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// Built-in layout name or user-defined table name (default: "romaji")
    pub kana_layout: String,
    /// Path to a user-defined kana table file (used when kana_layout is not a built-in name)
    pub kana_table: Option<PathBuf>,
    /// Default input mode on startup (default: "hiragana")
    pub default_mode: String,
    /// Key combinations that toggle IME on/off (default: ["shift+space"])
    pub toggle_keys: Vec<String>,
    /// Key combinations that toggle raw mode.  While raw mode is active, every key
    /// is passed straight through to the application except these toggle keys, so
    /// control keys the IME would consume (e.g. C-q / XON) can reach it.
    /// Default: [] (disabled until configured).  Example: ["ctrl+alt+p"].
    pub raw_mode_toggle_keys: Vec<String>,
    /// Characters that trigger immediate conversion in ▽ mode.
    /// Each entry must be a single character (e.g. [",", ".", "を"]).
    /// After the candidate is committed, the trigger character is also output.
    /// Default: [",", ".", "を"].  Set to [] to disable.
    pub conversion_trigger_chars: Vec<String>,
    /// vi-compatible behaviour: when true, Esc in a normal input phase
    /// (Hiragana / Katakana / HalfWidthKatakana / WideAscii / Ascii)
    /// switches the IME to Ascii mode.  Conversion phases (▽/▼ etc.) keep
    /// their normal cancel semantics.  Default: false.
    pub vi_escape: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            kana_layout: "romaji".into(),
            kana_table: None,
            default_mode: "ascii".into(),
            toggle_keys: vec!["shift+space".into()],
            raw_mode_toggle_keys: vec![],
            conversion_trigger_chars: vec![",".into(), ".".into(), "を".into()],
            vi_escape: false,
        }
    }
}

// ── [user-dict] ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserDictConfig {
    pub path: Option<PathBuf>,
}

impl UserDictConfig {
    /// Returns the effective user dict path, falling back to the XDG data dir default.
    pub fn effective_path(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("y2skk")
                .join("user.dict")
        })
    }
}

// ── [dict] ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictConfig {
    pub sources: Vec<DictSource>,
    /// When true, `(skk-ignore-dic-word ...)` directives in dictionaries are
    /// interpreted and the listed words are filtered from the candidate list.
    /// Set to false only if the parser has issues with a specific dictionary.
    /// Default: true.
    pub lisp_directives: bool,
}

impl Default for DictConfig {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            lisp_directives: true,
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictSource {
    pub path: PathBuf,
    #[serde_as(as = "DisplayFromStr")]
    #[serde(default = "default_encoding")]
    pub encoding: DictEncoding,
    #[serde(default)]
    pub priority: i32,
}

fn default_encoding() -> DictEncoding {
    DictEncoding::Utf8
}

// ── [indicator] ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorConfig {
    pub enabled: bool,
    /// Milliseconds before the indicator hides after a mode change (0 = always visible)
    pub timeout_ms: u32,
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 2000,
        }
    }
}

// ── [candidates] ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CandidatesConfig {
    /// Number of candidates shown inline before switching to list display
    pub inline_count: usize,
    /// Keys used to select candidates in list mode (left-to-right = candidate 1, 2, ...)
    pub selection_keys: String,
}

impl Default for CandidatesConfig {
    fn default() -> Self {
        Self {
            inline_count: 3,
            selection_keys: "asdfjkl;".into(),
        }
    }
}

// ── [daemon] ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub log_level: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            log_level: "info".into(),
        }
    }
}

// ── Loading ───────────────────────────────────────────────────────────────────

impl Config {
    /// Loads config from the given path.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let src = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let config: Self = toml::from_str(&src).map_err(|e| ConfigError::Toml {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(config.normalize())
    }

    /// Loads config from the default XDG location, returning defaults if the file does not exist.
    pub fn load_default() -> Self {
        let path = default_config_path();
        match Self::load(&path) {
            Ok(c) => c,
            Err(ConfigError::Io { .. }) => Self::default(),
            Err(e) => {
                tracing::warn!("Failed to load config from {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Expands `~` in all path fields.
    fn normalize(mut self) -> Self {
        if let Some(p) = self.input.kana_table {
            self.input.kana_table = Some(expand_tilde(p));
        }
        if let Some(p) = self.user_dict.path {
            self.user_dict.path = Some(expand_tilde(p));
        }
        for source in &mut self.dict.sources {
            source.path = expand_tilde(source.path.clone());
        }
        self
    }
}

// ── Kana table resolution ─────────────────────────────────────────────────────

/// Error returned when the kana conversion table cannot be located or parsed.
#[derive(Debug, thiserror::Error)]
pub enum KanaTableResolveError {
    #[error("kana table file not found for layout {layout:?}; searched: {searched}")]
    NotFound { layout: String, searched: String },
    #[error("kana table path {path} does not exist")]
    ExplicitPathNotFound { path: PathBuf },
    #[error("failed to parse kana table {path}: {source}")]
    Parse {
        path: PathBuf,
        source: skk_core::kana::parser::KanaTableError,
    },
}

/// Returns XDG-based directories in which to search for kana table files.
///
/// Search order (highest to lowest priority):
/// 1. `$XDG_CONFIG_HOME/y2skk/tables/` (default `~/.config/y2skk/tables/`)
/// 2. `$XDG_DATA_HOME/y2skk/tables/`   (default `~/.local/share/y2skk/tables/`)
/// 3. Each dir in `$XDG_DATA_DIRS` + `y2skk/tables/` (default `/usr/local/share:/usr/share`)
pub fn kana_table_search_dirs() -> Vec<PathBuf> {
    let mut dirs_list = Vec::new();

    // XDG_CONFIG_HOME (or ~/.config)
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config")));
    dirs_list.push(config_home.join("y2skk/tables"));

    // XDG_DATA_HOME (or ~/.local/share)
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::data_dir().unwrap_or_else(|| PathBuf::from(".local/share")));
    dirs_list.push(data_home.join("y2skk/tables"));

    // XDG_DATA_DIRS (or /usr/local/share:/usr/share)
    let data_dirs_str = env::var_os("XDG_DATA_DIRS")
        .unwrap_or_else(|| OsStr::new("/usr/local/share:/usr/share").to_os_string());
    for part in env::split_paths(&data_dirs_str) {
        dirs_list.push(part.join("y2skk/tables"));
    }

    dirs_list
}

/// Resolves the kana table path from `[input]` config, then loads and returns it.
///
/// - If `input.kana_table` is set, use that path directly (must exist).
/// - Otherwise, search for `<kana_layout>.txt` in the XDG table directories.
/// - Returns `Err` if the file is not found or fails to parse.
pub fn load_kana_table(
    input: &InputConfig,
) -> Result<skk_core::kana::table::KanaTable, KanaTableResolveError> {
    use skk_core::kana::parser::load_table;

    let path = if let Some(explicit) = &input.kana_table {
        if !explicit.exists() {
            return Err(KanaTableResolveError::ExplicitPathNotFound {
                path: explicit.clone(),
            });
        }
        explicit.clone()
    } else {
        let filename = format!("{}.txt", input.kana_layout);
        let search_dirs = kana_table_search_dirs();
        let found = search_dirs
            .iter()
            .map(|d| d.join(&filename))
            .find(|p| p.exists());
        match found {
            Some(p) => p,
            None => {
                let searched = search_dirs
                    .iter()
                    .map(|d| d.join(&filename).display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(KanaTableResolveError::NotFound {
                    layout: input.kana_layout.clone(),
                    searched,
                });
            }
        }
    };

    load_table(&path).map_err(|e| KanaTableResolveError::Parse {
        path: path.clone(),
        source: e,
    })
}

/// Returns the default config file path: `$XDG_CONFIG_HOME/y2skk/config.toml`.
pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("y2skk")
        .join("config.toml")
}

// ── Validation ────────────────────────────────────────────────────────────────

/// Error produced when the configuration contains invalid or unusable values.
#[derive(Debug, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("invalid default_mode {value:?} (expected: ascii / hiragana / katakana / half-width-katakana / wide-ascii)")]
    InvalidDefaultMode { value: String },
    #[error("invalid toggle key {value:?}: {reason}")]
    InvalidToggleKey { value: String, reason: String },
    #[error("conversion_trigger_chars entry {value:?} must be a single character")]
    InvalidConversionTriggerChar { value: String },
    #[error("kana table error: {0}")]
    KanaTable(#[from] KanaTableResolveError),
    #[error("dict source path does not exist: {path}")]
    DictPathNotFound { path: PathBuf },
    #[error("user dict parent directory does not exist: {path}")]
    UserDictParentNotFound { path: PathBuf },
}

/// Parses `default_mode` strictly, returning `Err` for unrecognised values.
pub fn parse_default_mode(name: &str) -> Result<skk_core::engine::SkkPhase, ConfigValidationError> {
    use skk_core::engine::SkkPhase;
    match name {
        "hiragana" => Ok(SkkPhase::Hiragana),
        "katakana" => Ok(SkkPhase::Katakana),
        "half-width-katakana" | "halfwidth-katakana" => Ok(SkkPhase::HalfWidthKatakana),
        "wide-ascii" | "wideascii" | "zenkaku" => Ok(SkkPhase::WideAscii),
        "ascii" | "latin" => Ok(SkkPhase::Ascii),
        other => Err(ConfigValidationError::InvalidDefaultMode {
            value: other.to_string(),
        }),
    }
}

/// Parses a toggle key string strictly, returning `Err` for unrecognised values.
pub fn parse_toggle_key(
    s: &str,
) -> Result<(skk_core::key::Key, skk_core::key::Modifiers), ConfigValidationError> {
    use skk_core::key::{Key, Modifiers};
    let parts: Vec<&str> = s.split('+').collect();
    let key_str = parts
        .last()
        .ok_or_else(|| ConfigValidationError::InvalidToggleKey {
            value: s.to_string(),
            reason: "empty string".to_string(),
        })?;
    let key = match key_str.to_lowercase().as_str() {
        "space" => Key::Space,
        "return" | "enter" => Key::Return,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        k if k.chars().count() == 1 => Key::Char(k.chars().next().unwrap()),
        _ => {
            return Err(ConfigValidationError::InvalidToggleKey {
                value: s.to_string(),
                reason: format!("unrecognised key name {:?}", key_str),
            })
        }
    };
    let mut mods = Modifiers::empty();
    for mod_str in &parts[..parts.len() - 1] {
        match mod_str.to_lowercase().as_str() {
            "shift" => mods |= Modifiers::SHIFT,
            "ctrl" | "control" => mods |= Modifiers::CTRL,
            "alt" => mods |= Modifiers::ALT,
            "meta" | "super" => mods |= Modifiers::META,
            other => {
                return Err(ConfigValidationError::InvalidToggleKey {
                    value: s.to_string(),
                    reason: format!("unrecognised modifier {:?}", other),
                })
            }
        }
    }
    Ok((key, mods))
}

/// Fully validates a loaded [`Config`], checking value correctness and path
/// existence.  Returns the first error encountered, or `Ok(())` if the config
/// is usable to start the daemon.
pub fn validate(config: &Config) -> Result<(), ConfigValidationError> {
    // 1. default_mode
    parse_default_mode(&config.input.default_mode)?;

    // 2. toggle_keys — all entries must parse
    for key in &config.input.toggle_keys {
        parse_toggle_key(key)?;
    }

    // 2b. raw_mode_toggle_keys — all entries must parse
    for key in &config.input.raw_mode_toggle_keys {
        parse_toggle_key(key)?;
    }

    // 3. conversion_trigger_chars — each must be a single character
    for entry in &config.input.conversion_trigger_chars {
        let mut chars = entry.chars();
        if chars.next().is_none() || chars.next().is_some() {
            return Err(ConfigValidationError::InvalidConversionTriggerChar {
                value: entry.clone(),
            });
        }
    }

    // 4. kana table — existence + parse
    load_kana_table(&config.input)?;

    // 5. dict sources — each path must exist
    for source in &config.dict.sources {
        if !source.path.exists() {
            return Err(ConfigValidationError::DictPathNotFound {
                path: source.path.clone(),
            });
        }
    }

    // 6. user dict — parent directory must exist (the file itself is created on demand)
    let user_dict_path = config.user_dict.effective_path();
    if let Some(parent) = user_dict_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(ConfigValidationError::UserDictParentNotFound {
                path: parent.to_path_buf(),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_with_slash() {
        if let Some(home) = dirs::home_dir() {
            let p = expand_tilde(PathBuf::from("~/foo/bar"));
            assert_eq!(p, home.join("foo/bar"));
        }
    }

    #[test]
    fn expand_tilde_alone() {
        if let Some(home) = dirs::home_dir() {
            let p = expand_tilde(PathBuf::from("~"));
            assert_eq!(p, home);
        }
    }

    #[test]
    fn expand_tilde_no_tilde() {
        let p = expand_tilde(PathBuf::from("/usr/share/skk/SKK-JISYO.L"));
        assert_eq!(p, PathBuf::from("/usr/share/skk/SKK-JISYO.L"));
    }

    #[test]
    fn normalize_expands_user_dict_path() {
        if let Some(home) = dirs::home_dir() {
            let toml = r#"
[user-dict]
path = "~/.local/share/y2skk/user.dict"
"#;
            let config: Config = toml::from_str(toml).unwrap();
            let config = config.normalize();
            assert_eq!(
                config.user_dict.path.unwrap(),
                home.join(".local/share/y2skk/user.dict"),
            );
        }
    }

    #[test]
    fn normalize_expands_dict_source_path() {
        if let Some(home) = dirs::home_dir() {
            let toml = r#"
[[dict.sources]]
path = "~/dicts/SKK-JISYO.L"
encoding = "utf-8"
priority = 0
"#;
            let config: Config = toml::from_str(toml).unwrap();
            let config = config.normalize();
            assert_eq!(config.dict.sources[0].path, home.join("dicts/SKK-JISYO.L"),);
        }
    }

    #[test]
    fn resolve_kana_table_via_xdg_data_home() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tables_dir = tmp.path().join("y2skk/tables");
        std::fs::create_dir_all(&tables_dir).unwrap();
        let table_file = tables_dir.join("romaji.txt");
        std::fs::copy("../../dist/tables/romaji.txt", &table_file).unwrap();

        // Override XDG_DATA_HOME so the search finds our temp dir.
        // Also clear XDG_CONFIG_HOME to avoid any accidental match there.
        let provider = env_guard::Provider::get();
        let _guard = provider.set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
        let _guard2 = provider.set("XDG_CONFIG_HOME", "/nonexistent");
        let _guard3 = provider.set("XDG_DATA_DIRS", "/nonexistent");

        let input = InputConfig {
            kana_layout: "romaji".into(),
            ..InputConfig::default()
        };
        let result = load_kana_table(&input);
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn resolve_kana_table_not_found() {
        let provider = env_guard::Provider::get();
        let _guard = provider.set("XDG_DATA_HOME", "/nonexistent");
        let _guard2 = provider.set("XDG_CONFIG_HOME", "/nonexistent");
        let _guard3 = provider.set("XDG_DATA_DIRS", "/nonexistent");

        let input = InputConfig {
            kana_layout: "romaji".into(),
            ..InputConfig::default()
        };
        let result = load_kana_table(&input);
        assert!(matches!(
            result,
            Err(KanaTableResolveError::NotFound { .. })
        ));
    }

    #[test]
    fn resolve_kana_table_explicit_path() {
        // Absolute path to the dist table (works from crate root during `cargo test`)
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let path = manifest.join("../../dist/tables/romaji.txt");
        let input = InputConfig {
            kana_layout: "romaji".into(),
            kana_table: Some(path),
            ..InputConfig::default()
        };
        let result = load_kana_table(&input);
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn resolve_kana_table_explicit_path_missing() {
        let input = InputConfig {
            kana_layout: "romaji".into(),
            kana_table: Some(PathBuf::from("/nonexistent/romaji.txt")),
            ..InputConfig::default()
        };
        let result = load_kana_table(&input);
        assert!(matches!(
            result,
            Err(KanaTableResolveError::ExplicitPathNotFound { .. })
        ));
    }

    // ── Validation tests ─────────────────────────────────────────────────────

    #[test]
    fn validate_toml_syntax_error() {
        let toml = "this is not valid = toml =[";
        let err = toml::from_str::<Config>(toml);
        assert!(err.is_err(), "expected TOML parse error");
    }

    #[test]
    fn validate_invalid_default_mode() {
        let result = parse_default_mode("bar");
        assert!(matches!(
            result,
            Err(ConfigValidationError::InvalidDefaultMode { .. })
        ));
    }

    #[test]
    fn validate_valid_default_modes() {
        for mode in &[
            "ascii",
            "hiragana",
            "katakana",
            "half-width-katakana",
            "wide-ascii",
        ] {
            assert!(parse_default_mode(mode).is_ok(), "expected Ok for {mode}");
        }
    }

    #[test]
    fn validate_invalid_toggle_key_modifier() {
        let result = parse_toggle_key("bogusmod+space");
        assert!(matches!(
            result,
            Err(ConfigValidationError::InvalidToggleKey { .. })
        ));
    }

    #[test]
    fn validate_invalid_toggle_key_name() {
        let result = parse_toggle_key("shift+boguskey");
        assert!(matches!(
            result,
            Err(ConfigValidationError::InvalidToggleKey { .. })
        ));
    }

    #[test]
    fn validate_valid_toggle_key() {
        assert!(parse_toggle_key("shift+space").is_ok());
        assert!(parse_toggle_key("ctrl+a").is_ok());
    }

    #[test]
    fn validate_invalid_conversion_trigger_char() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let table_path = manifest.join("../../dist/tables/romaji.txt");
        let config = Config {
            input: InputConfig {
                kana_table: Some(table_path),
                conversion_trigger_chars: vec!["ab".to_string()],
                ..InputConfig::default()
            },
            ..Config::default()
        };
        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ConfigValidationError::InvalidConversionTriggerChar { .. })
        ));
    }

    #[test]
    fn validate_invalid_raw_mode_toggle_key() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let table_path = manifest.join("../../dist/tables/romaji.txt");
        let config = Config {
            input: InputConfig {
                kana_table: Some(table_path),
                raw_mode_toggle_keys: vec!["shift+boguskey".to_string()],
                ..InputConfig::default()
            },
            ..Config::default()
        };
        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ConfigValidationError::InvalidToggleKey { .. })
        ));
    }

    #[test]
    fn dict_config_changes_detected_for_reload() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let table_path = manifest.join("../../dist/tables/romaji.txt");
        let base = Config {
            input: InputConfig {
                kana_table: Some(table_path),
                ..InputConfig::default()
            },
            ..Config::default()
        };

        // Changing only [candidates] must NOT count as a dictionary change.
        let mut only_candidates = base.clone();
        only_candidates.candidates.inline_count += 1;
        assert_eq!(base.dict.sources, only_candidates.dict.sources);
        assert_eq!(base.user_dict, only_candidates.user_dict);

        // Changing dict sources or the user-dict path IS a dictionary change.
        let mut diff_dict = base.clone();
        diff_dict.dict.sources.push(DictSource {
            path: PathBuf::from("/tmp/x.dict"),
            encoding: DictEncoding::Utf8,
            priority: 0,
        });
        assert_ne!(base.dict.sources, diff_dict.dict.sources);

        let mut diff_userdict = base.clone();
        diff_userdict.user_dict.path = Some(PathBuf::from("/tmp/u.dict"));
        assert_ne!(base.user_dict, diff_userdict.user_dict);
    }

    #[test]
    fn validate_valid_raw_mode_toggle_key() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let table_path = manifest.join("../../dist/tables/romaji.txt");
        let config = Config {
            input: InputConfig {
                kana_table: Some(table_path),
                raw_mode_toggle_keys: vec!["ctrl+alt+p".to_string()],
                ..InputConfig::default()
            },
            // Point the user dict at an existing directory (the crate root) so the
            // parent-exists check passes on a clean machine (e.g. CI) where the
            // default ~/.local/share/y2skk/ does not exist.
            user_dict: UserDictConfig {
                path: Some(manifest.join("user.dict")),
            },
            ..Config::default()
        };
        assert!(validate(&config).is_ok());
    }

    #[test]
    fn validate_dict_path_not_found() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let table_path = manifest.join("../../dist/tables/romaji.txt");
        let config = Config {
            input: InputConfig {
                kana_table: Some(table_path),
                ..InputConfig::default()
            },
            dict: DictConfig {
                sources: vec![DictSource {
                    path: PathBuf::from("/nonexistent/SKK-JISYO.L"),
                    encoding: DictEncoding::Utf8,
                    priority: 0,
                }],
                ..DictConfig::default()
            },
            ..Config::default()
        };
        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ConfigValidationError::DictPathNotFound { .. })
        ));
    }

    // ── Helper: temporarily override an environment variable ─────────────────

    mod env_guard {
        use std::{
            env,
            sync::{Mutex, MutexGuard},
        };

        pub struct Provider;

        impl Provider {
            pub fn get() -> MutexGuard<'static, Self> {
                ENV_GUARD_PROVIDER_MUTEX
                    .lock()
                    .expect("should be able to lock mutex")
            }

            pub fn set(&self, key: &str, val: &str) -> EnvGuard {
                EnvGuard::set(key, val)
            }
        }

        static ENV_GUARD_PROVIDER_MUTEX: Mutex<Provider> = Mutex::new(Provider);

        pub struct EnvGuard {
            key: String,
            original: Option<std::ffi::OsString>,
        }
        impl EnvGuard {
            fn set(key: &str, val: &str) -> Self {
                let original = env::var_os(key);
                // SAFETY: this method called only via Provider::set, which requires locking the
                // mutex, so concurrent calls are prevented.
                unsafe { env::set_var(key, val) };
                Self {
                    key: key.to_string(),
                    original,
                }
            }
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                // SAFETY: same as in EnvGuard::set.
                match &self.original {
                    Some(v) => unsafe { env::set_var(&self.key, v) },
                    None => unsafe { env::remove_var(&self.key) },
                }
            }
        }
    }
}
