use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use skk_core::config::SkkKeybindings;
use skk_core::dict::file::{DictEncoding, FileDict, UserDict};
use skk_core::dict::traits::DictionaryProvider;
use skk_core::engine::{EngineAction, SkkEngine, SkkPhase};
use skk_core::kana::builtin::builtin_table;
use skk_core::kana::table::KanaLayout;
use skk_ipc::{IpcAction, SessionId};

use crate::config::Config;

/// Manages all active IME sessions and the shared dictionary stack.
///
/// Each client gets its own `SkkEngine` so input state is isolated per
/// application, but all sessions share the same loaded dictionaries.
pub struct SessionManager {
    sessions: HashMap<SessionId, SkkEngine>,
    next_id: SessionId,
    layout: KanaLayout,
    keybindings: SkkKeybindings,
    initial_phase: SkkPhase,
    /// Shared read-only dictionaries (system dicts, loaded once at startup).
    dicts: Vec<Arc<dyn DictionaryProvider>>,
    /// Writable user dictionary (exclusive to the daemon).
    user_dict: Arc<Mutex<UserDict>>,
}

impl SessionManager {
    pub fn new(config: &Config) -> Self {
        let layout = parse_layout(&config.input.kana_layout);
        let (dicts, user_dict) = load_dicts(config);
        let keybindings = keybindings_from_config(config);
        let initial_phase = parse_default_mode(&config.input.default_mode);
        Self {
            sessions: HashMap::new(),
            next_id: 1,
            layout,
            keybindings,
            initial_phase,
            dicts,
            user_dict,
        }
    }

    /// Creates a new session and returns its ID.
    pub fn create_session(&mut self, _app_id: &str) -> SessionId {
        let id = self.next_id;
        self.next_id += 1;

        let table = builtin_table(self.layout);
        let mut engine = SkkEngine::new(table, self.keybindings.clone())
            .with_initial_phase(self.initial_phase.clone());

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
            return vec![IpcAction::passthrough()];
        };

        let event = skk_ipc::convert::key_event_from_raw(key_sym, modifiers, is_press);
        let actions: Vec<EngineAction> = engine.process_key(&event);
        let ipc_actions: Vec<IpcAction> = actions.into_iter().map(IpcAction::from).collect();

        // Persist the user dict whenever a conversion is committed.
        if ipc_actions.iter().any(|a| a.kind == skk_ipc::ACTION_COMMIT) {
            self.flush_user_dict();
        }

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
        let encoding = DictEncoding::from_str(&source.encoding);
        match FileDict::load(&source.path, encoding, source.priority) {
            Ok(d) => {
                tracing::info!("Loaded dict {} (priority {})", source.path.display(), source.priority);
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
    let conversion_trigger_chars: Vec<char> = config
        .input
        .conversion_trigger_chars
        .iter()
        .filter_map(|s| {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_none() { Some(c) } else { None }
        })
        .collect();

    SkkKeybindings {
        inline_count: config.candidates.inline_count,
        selection_keys: config.candidates.selection_keys.chars().collect(),
        conversion_trigger_chars,
        ..SkkKeybindings::default()
    }
}

fn parse_default_mode(name: &str) -> SkkPhase {
    match name {
        "katakana" => SkkPhase::Katakana,
        "half-width-katakana" | "halfwidth-katakana" => SkkPhase::HalfWidthKatakana,
        "wide-ascii" | "wideascii" | "zenkaku" => SkkPhase::WideAscii,
        "ascii" | "latin" => SkkPhase::Ascii,
        _ => SkkPhase::Hiragana,
    }
}

fn parse_layout(name: &str) -> KanaLayout {
    match name {
        "azik" | "azik-us" => KanaLayout::AzikUs,
        "azik-jp" => KanaLayout::AzikJp,
        "dvorakjp" | "dvorakjp-us" => KanaLayout::DvorakJpUs,
        "dvorakjp-jp" => KanaLayout::DvorakJpJp,
        _ => KanaLayout::Romaji,
    }
}

// ── Arc wrappers so shared dicts satisfy DictionaryProvider ──────────────────

/// Wraps a shared read-only dict behind Arc for use in multiple engines.
struct SharedDict(Arc<dyn DictionaryProvider>);

impl DictionaryProvider for SharedDict {
    fn lookup(&self, midashi: &str, okuri: Option<&str>) -> Option<skk_core::dict::DictEntry> {
        self.0.lookup(midashi, okuri)
    }
    fn learn(&mut self, _entry: skk_core::dict::DictEntry) -> Result<(), skk_core::dict::DictError> {
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
        self.0.lock()
            .map_err(|_| skk_core::dict::DictError::ReadOnly)?
            .learn(entry)
    }
    fn priority(&self) -> i32 {
        i32::MAX
    }
    fn complete(&self, prefix: &str) -> Vec<String> {
        self.0.lock().map(|ud| ud.complete(prefix)).unwrap_or_default()
    }
}
