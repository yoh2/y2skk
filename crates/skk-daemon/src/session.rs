use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use skk_core::config::SkkKeybindings;
use skk_core::dict::file::{FileDict, UserDict};
use skk_core::dict::traits::DictionaryProvider;
use skk_core::engine::{EngineAction, SkkEngine, SkkPhase};
use skk_core::kana::table::KanaTable;
use skk_core::key::{Key, Modifiers};
use skk_ipc::{IpcAction, SessionId, ACTION_SESSION_INVALID};

use crate::config::Config;

/// Manages all active IME sessions and the shared dictionary stack.
///
/// Each client gets its own `SkkEngine` so input state is isolated per
/// application, but all sessions share the same loaded dictionaries.
pub struct SessionManager {
    sessions: HashMap<SessionId, SkkEngine>,
    next_id: SessionId,
    /// Loaded kana conversion table shared by all sessions.
    kana_table: KanaTable,
    keybindings: SkkKeybindings,
    initial_phase: SkkPhase,
    /// Shared read-only dictionaries (system dicts, loaded once at startup).
    dicts: Vec<Arc<dyn DictionaryProvider>>,
    /// Writable user dictionary (exclusive to the daemon).
    user_dict: Arc<Mutex<UserDict>>,
    /// Indicator auto-hide timeout in milliseconds (0 = always visible).
    indicator_timeout_ms: u32,
    /// Whether Lisp-form directives (e.g. `skk-ignore-dic-word`) are interpreted.
    lisp_directives: bool,
}

impl SessionManager {
    pub fn new(config: &Config) -> Result<Self, crate::config::KanaTableResolveError> {
        let kana_table = crate::config::load_kana_table(&config.input)?;
        let (dicts, user_dict) = load_dicts(config);
        let keybindings = keybindings_from_config(config);
        // Config has been validated before SessionManager::new is called.
        let initial_phase = crate::config::parse_default_mode(&config.input.default_mode)
            .unwrap_or(skk_core::engine::SkkPhase::Hiragana);
        Ok(Self {
            sessions: HashMap::new(),
            next_id: 1,
            kana_table,
            keybindings,
            initial_phase,
            dicts,
            user_dict,
            indicator_timeout_ms: if config.indicator.enabled {
                config.indicator.timeout_ms
            } else {
                0
            },
            lisp_directives: config.dict.lisp_directives,
        })
    }

    /// Creates a new session and returns its ID.
    pub fn create_session(&mut self, _app_id: &str) -> SessionId {
        let id = self.next_id;
        self.next_id += 1;

        let mut engine = SkkEngine::new(self.kana_table.clone(), self.keybindings.clone())
            .with_initial_phase(self.initial_phase.clone())
            .with_lisp_directives(self.lisp_directives);

        // Attach shared dictionaries (highest-priority first).
        // User dict is always first.
        engine.add_dict(Box::new(SharedUserDict(Arc::clone(&self.user_dict))));
        for dict in &self.dicts {
            engine.add_dict(Box::new(SharedDict(Arc::clone(dict))));
        }

        self.sessions.insert(id, engine);
        tracing::debug!("Session {id} created");
        id
    }

    /// Destroys a session and saves the user dict if it is dirty.
    pub fn destroy_session(&mut self, session_id: SessionId) {
        if self.sessions.remove(&session_id).is_some() {
            tracing::debug!("Session {session_id} destroyed");
        } else {
            tracing::warn!("destroy_session: unknown session {session_id}");
        }
        self.flush_user_dict();
    }

    /// Processes a key event for the given session and returns IPC actions.
    pub fn process_key(
        &mut self,
        session_id: SessionId,
        key_sym: u32,
        modifiers: u32,
        is_press: bool,
    ) -> Vec<IpcAction> {
        let Some(engine) = self.sessions.get_mut(&session_id) else {
            tracing::warn!("process_key: unknown session {session_id}");
            return vec![IpcAction {
                kind: ACTION_SESSION_INVALID,
                text: String::new(),
                cursor: 0,
                candidates: vec![],
                focused: 0,
                ghost_start: skk_ipc::NO_GHOST,
            }];
        };

        let event = skk_ipc::convert::key_event_from_raw(key_sym, modifiers, is_press);
        let actions: Vec<EngineAction> = engine.process_key(&event);
        let timeout_ms = self.indicator_timeout_ms;
        let ipc_actions: Vec<IpcAction> = actions
            .into_iter()
            .map(IpcAction::from)
            .filter(|a| {
                // Drop UpdateStatus when the indicator is disabled (timeout_ms == 0
                // means disabled; adapters would show without ever hiding it).
                !(a.kind == skk_ipc::ACTION_UPDATE_STATUS && timeout_ms == 0)
            })
            .map(|mut a| {
                // Pack indicator timeout into the unused cursor field so adapters
                // can auto-hide the status window without knowing the config.
                if a.kind == skk_ipc::ACTION_UPDATE_STATUS {
                    a.cursor = timeout_ms;
                }
                a
            })
            .collect();

        // Persist the user dict whenever it has been mutated (commit, purge, etc.).
        // `save()` is a no-op when the dict is not dirty, so this is cheap.
        self.flush_user_dict();

        ipc_actions
    }

    /// Saves the user dict to disk if it has been modified.
    pub fn flush_user_dict(&self) {
        if let Ok(mut ud) = self.user_dict.lock() {
            if let Err(e) = ud.save() {
                tracing::error!("Failed to save user dict: {e}");
            }
        }
    }

    /// Applies a new (already-validated) config to the running daemon.
    ///
    /// The kana table is rebuilt first; if that fails the current state is left
    /// untouched and the error is returned (validate-before-apply).  On success,
    /// every live session's engine is rebuilt with the new table and keybindings,
    /// preserving only its base input mode — any in-progress conversion,
    /// completion and raw-mode state is dropped.  Session IDs are preserved, so
    /// the reload is transparent to adapters (no reconnect).
    ///
    /// Dictionaries are reloaded only when `dicts_changed` is true (the user dict
    /// is flushed first); otherwise the existing shared dictionaries are kept.
    /// Dictionary loading is intentionally tolerant, matching daemon startup: a
    /// source that fails to load is logged and skipped, and a missing user dict
    /// falls back to an empty in-memory one.  Such failures do not abort the
    /// reload (the kana table is the only strictly validated resource here).
    pub fn reload(
        &mut self,
        config: &Config,
        dicts_changed: bool,
    ) -> Result<(), crate::config::KanaTableResolveError> {
        // Build new resources first; bail out (keeping current state) on failure.
        let kana_table = crate::config::load_kana_table(&config.input)?;
        let keybindings = keybindings_from_config(config);
        let initial_phase = crate::config::parse_default_mode(&config.input.default_mode)
            .unwrap_or(SkkPhase::Hiragana);
        let indicator_timeout_ms = if config.indicator.enabled {
            config.indicator.timeout_ms
        } else {
            0
        };
        let lisp_directives = config.dict.lisp_directives;

        if dicts_changed {
            self.flush_user_dict();
            let (dicts, user_dict) = load_dicts(config);
            self.dicts = dicts;
            self.user_dict = user_dict;
        }
        self.kana_table = kana_table;
        self.keybindings = keybindings;
        self.initial_phase = initial_phase;
        self.indicator_timeout_ms = indicator_timeout_ms;
        self.lisp_directives = lisp_directives;

        // Rebuild each live engine, preserving only its base input mode.
        let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in ids {
            let mode = self.sessions[&id].base_input_mode();
            let mut engine = SkkEngine::new(self.kana_table.clone(), self.keybindings.clone())
                .with_initial_phase(mode)
                .with_lisp_directives(self.lisp_directives);
            engine.add_dict(Box::new(SharedUserDict(Arc::clone(&self.user_dict))));
            for dict in &self.dicts {
                engine.add_dict(Box::new(SharedDict(Arc::clone(dict))));
            }
            self.sessions.insert(id, engine);
        }
        tracing::info!(
            "Applied reloaded config to {} session(s)",
            self.sessions.len()
        );
        Ok(())
    }
}

// ── Dictionary loading ────────────────────────────────────────────────────────

fn load_dicts(config: &Config) -> (Vec<Arc<dyn DictionaryProvider>>, Arc<Mutex<UserDict>>) {
    // User dict
    let user_dict_path = config.user_dict.effective_path();
    let user_dict = match UserDict::load(&user_dict_path) {
        Ok(ud) => ud,
        Err(e) => {
            tracing::warn!("Failed to load user dict {}: {e}", user_dict_path.display());
            UserDict::load("/dev/null").unwrap_or_else(|_| {
                // Construct an empty in-memory user dict as fallback
                UserDict::empty(user_dict_path.clone())
            })
        }
    };

    // System dicts
    let mut dicts: Vec<Arc<dyn DictionaryProvider>> = Vec::new();
    for source in &config.dict.sources {
        match FileDict::load(&source.path, source.encoding, source.priority) {
            Ok(d) => {
                tracing::info!(
                    "Loaded dict {} (priority {})",
                    source.path.display(),
                    source.priority
                );
                dicts.push(Arc::new(d));
            }
            Err(e) => {
                tracing::warn!("Failed to load dict {}: {e}", source.path.display());
            }
        }
    }

    (dicts, Arc::new(Mutex::new(user_dict)))
}

fn keybindings_from_config(config: &Config) -> SkkKeybindings {
    // Config has already been validated before SessionManager::new is called,
    // so these unwraps are safe.
    let conversion_trigger_chars: Vec<char> = config
        .input
        .conversion_trigger_chars
        .iter()
        .filter_map(|s| s.chars().next())
        .collect();

    let toggle_keys: Vec<(Key, Modifiers)> = config
        .input
        .toggle_keys
        .iter()
        .filter_map(|s| crate::config::parse_toggle_key(s).ok())
        .collect();

    let raw_mode_toggle_keys: Vec<(Key, Modifiers)> = config
        .input
        .raw_mode_toggle_keys
        .iter()
        .filter_map(|s| crate::config::parse_toggle_key(s).ok())
        .collect();

    SkkKeybindings {
        inline_count: config.candidates.inline_count,
        selection_keys: config.candidates.selection_keys.chars().collect(),
        conversion_trigger_chars,
        toggle_keys,
        raw_mode_toggle_keys,
        vi_escape: config.input.vi_escape,
        ..SkkKeybindings::default()
    }
}

// ── Arc wrappers so shared dicts satisfy DictionaryProvider ──────────────────

/// Wraps a shared read-only dict behind Arc for use in multiple engines.
struct SharedDict(Arc<dyn DictionaryProvider>);

impl DictionaryProvider for SharedDict {
    fn lookup(&self, midashi: &str, okuri: Option<&str>) -> Option<skk_core::dict::DictEntry> {
        self.0.lookup(midashi, okuri)
    }
    fn learn(
        &mut self,
        _entry: skk_core::dict::DictEntry,
    ) -> Result<(), skk_core::dict::DictError> {
        Err(skk_core::dict::DictError::ReadOnly)
    }
    fn priority(&self) -> i32 {
        self.0.priority()
    }
    fn complete(&self, prefix: &str) -> Vec<String> {
        self.0.complete(prefix)
    }
}

/// Wraps the shared writable user dict behind Arc<Mutex<>>.
struct SharedUserDict(Arc<Mutex<UserDict>>);

impl DictionaryProvider for SharedUserDict {
    fn lookup(&self, midashi: &str, okuri: Option<&str>) -> Option<skk_core::dict::DictEntry> {
        self.0.lock().ok()?.lookup(midashi, okuri)
    }
    fn learn(&mut self, entry: skk_core::dict::DictEntry) -> Result<(), skk_core::dict::DictError> {
        self.0
            .lock()
            .map_err(|_| skk_core::dict::DictError::ReadOnly)?
            .learn(entry)
    }
    fn purge(
        &mut self,
        midashi: &str,
        okuri: Option<&str>,
        word: &str,
    ) -> Result<bool, skk_core::dict::DictError> {
        self.0
            .lock()
            .map_err(|_| skk_core::dict::DictError::ReadOnly)?
            .purge(midashi, okuri, word)
    }
    fn priority(&self) -> i32 {
        i32::MAX
    }
    fn complete(&self, prefix: &str) -> Vec<String> {
        self.0
            .lock()
            .map(|ud| ud.complete(prefix))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, InputConfig, UserDictConfig};
    use std::path::PathBuf;

    fn romaji_table_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/tables/romaji.txt")
    }

    fn test_config(default_mode: &str, user_dict_path: &std::path::Path) -> Config {
        Config {
            input: InputConfig {
                kana_table: Some(romaji_table_path()),
                default_mode: default_mode.into(),
                ..InputConfig::default()
            },
            // Keep the user dict in a caller-owned temp dir (unique per run).
            user_dict: UserDictConfig {
                path: Some(user_dict_path.to_path_buf()),
            },
            ..Config::default()
        }
    }

    #[test]
    fn reload_preserves_session_ids_and_base_mode() {
        let dir = tempfile::tempdir().unwrap();
        let user_dict = dir.path().join("user.dict");
        let mut mgr = SessionManager::new(&test_config("hiragana", &user_dict)).unwrap();
        let s1 = mgr.create_session("a");
        let s2 = mgr.create_session("b");

        // s1: 'q' toggles Hiragana → Katakana. s2: 'A' starts a ▽ conversion.
        mgr.process_key(s1, 'q' as u32, 0, true);
        mgr.process_key(s2, 'A' as u32, 0, true);
        assert_eq!(mgr.sessions[&s1].phase(), &SkkPhase::Katakana);
        assert!(matches!(
            mgr.sessions[&s2].phase(),
            SkkPhase::Midashi { .. }
        ));

        // Reload with a different default_mode; dictionaries unchanged.
        mgr.reload(&test_config("ascii", &user_dict), false)
            .unwrap();

        // Session IDs preserved (transparent to adapters); base mode kept;
        // the in-progress ▽ collapses to its base mode (Hiragana), not default.
        assert!(mgr.sessions.contains_key(&s1));
        assert!(mgr.sessions.contains_key(&s2));
        assert_eq!(mgr.sessions[&s1].phase(), &SkkPhase::Katakana);
        assert_eq!(mgr.sessions[&s2].phase(), &SkkPhase::Hiragana);
    }
}
