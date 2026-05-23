use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use zbus::interface;

use skk_ipc::{IpcAction, SessionId};

use crate::config::{self, Config};
use crate::session::SessionManager;

// ── D-Bus interface ───────────────────────────────────────────────────────────

/// The main D-Bus interface object for y2skk.
///
/// Bus name  : `org.y2skk.Daemon`
/// Object path: `/org/y2skk/Daemon`
pub struct DaemonInterface {
    sessions: Arc<Mutex<SessionManager>>,
    config: Arc<Mutex<Config>>,
    /// Path of the loaded config file, used for ReloadConfig.
    config_path: PathBuf,
}

impl DaemonInterface {
    pub fn new(
        sessions: Arc<Mutex<SessionManager>>,
        config: Arc<Mutex<Config>>,
        config_path: PathBuf,
    ) -> Self {
        Self {
            sessions,
            config,
            config_path,
        }
    }
}

#[interface(name = "org.y2skk.Daemon")]
impl DaemonInterface {
    /// Creates a new IME session for the given application identifier.
    /// Returns the session ID to be used in subsequent calls.
    async fn create_session(&self, app_id: &str) -> SessionId {
        self.sessions.lock().await.create_session(app_id)
    }

    /// Destroys an existing session, freeing its resources.
    async fn destroy_session(&self, session_id: SessionId) {
        self.sessions.lock().await.destroy_session(session_id);
    }

    /// Processes a key event and returns a list of actions for the adapter.
    ///
    /// `key_sym`   — X11 keysym value
    /// `modifiers` — X11 modifier bitmask (Shift=0x01, Ctrl=0x04, Alt=0x08, Meta=0x40)
    /// `is_press`  — `true` for key-press, `false` for key-release
    async fn process_key(
        &self,
        session_id: SessionId,
        key_sym: u32,
        modifiers: u32,
        is_press: bool,
    ) -> Vec<IpcAction> {
        self.sessions
            .lock()
            .await
            .process_key(session_id, key_sym, modifiers, is_press)
    }

    /// Reloads the configuration file from disk and applies it to all live
    /// sessions (validate-before-apply: on any load/validation/build error the
    /// current configuration is kept and the error is returned to the caller).
    ///
    /// Each session's engine is rebuilt with the new table/keybindings while its
    /// base input mode is preserved; session IDs are unchanged so adapters need
    /// not reconnect.  Dictionaries are reloaded only when the dict / user-dict
    /// configuration changed.
    async fn reload_config(&self) -> zbus::fdo::Result<()> {
        let new_cfg = Config::load(&self.config_path)
            .map_err(|e| zbus::fdo::Error::Failed(format!("config load failed: {e}")))?;
        // validate() builds the kana table; reuse it for the apply so the table
        // is parsed only once per reload.
        let kana_table = config::validate(&new_cfg)
            .map_err(|e| zbus::fdo::Error::Failed(format!("config validation failed: {e}")))?;

        // Hold the config lock across the whole apply so concurrent ReloadConfig
        // calls are serialized and `dicts_changed` is always compared against the
        // currently-applied config (no stale comparison / wrongly-skipped dict
        // reload).  No other path locks `sessions` before `config`, so acquiring
        // `sessions` while holding `config` cannot deadlock.
        let mut config_guard = self.config.lock().await;
        let dicts_changed = config_guard.dict.sources != new_cfg.dict.sources
            || config_guard.user_dict != new_cfg.user_dict;

        self.sessions
            .lock()
            .await
            .reload(&new_cfg, dicts_changed, kana_table);

        *config_guard = new_cfg;
        tracing::info!("Config reloaded from {}", self.config_path.display());
        Ok(())
    }
}

// ── Daemon startup ────────────────────────────────────────────────────────────

/// Starts the D-Bus service and runs until the process receives a shutdown signal.
pub async fn run(config: Config, config_path: PathBuf) -> anyhow::Result<()> {
    let sessions = Arc::new(Mutex::new(SessionManager::new(&config)?));
    let config = Arc::new(Mutex::new(config));

    let interface = DaemonInterface::new(sessions.clone(), config, config_path);

    let _conn = zbus::connection::Builder::session()?
        .name("org.y2skk.Daemon")?
        .serve_at("/org/y2skk/Daemon", interface)?
        .build()
        .await?;

    tracing::info!("y2skk-daemon started on session bus as org.y2skk.Daemon");

    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down (signal)");

    // Force-exit here because the zbus background task keeps its message-dispatch
    // loop running even after the signal is received, preventing the tokio runtime
    // from completing its graceful shutdown.  The daemon holds no state that
    // requires flushing at exit (user-dictionary writes happen at session
    // destruction, not here), so a hard exit is safe.
    std::process::exit(0);
}
