use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

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

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("TOML parse error in {path}: {source}")]
    Toml { path: PathBuf, source: toml::de::Error },
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
            conversion_trigger_chars: vec![",".into(), ".".into(), "を".into()],
            vi_escape: false,
        }
    }
}

// ── [user-dict] ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserDictConfig {
    pub path: Option<PathBuf>,
}

impl Default for UserDictConfig {
    fn default() -> Self {
        Self { path: None }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DictConfig {
    pub sources: Vec<DictSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictSource {
    pub path: PathBuf,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[serde(default)]
    pub priority: i32,
}

fn default_encoding() -> String {
    "utf-8".into()
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
        Self { enabled: true, timeout_ms: 2000 }
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
        Self { log_level: "info".into() }
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

/// Returns the default config file path: `$XDG_CONFIG_HOME/y2skk/config.toml`.
pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("y2skk")
        .join("config.toml")
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
            assert_eq!(
                config.dict.sources[0].path,
                home.join("dicts/SKK-JISYO.L"),
            );
        }
    }
}
