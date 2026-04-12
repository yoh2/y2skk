use std::sync::Arc;

use tokio::sync::Mutex;
use zbus::interface;

use skk_ipc::{IpcAction, SessionId};

use crate::config::Config;
use crate::session::SessionManager;

// ── D-Bus interface ───────────────────────────────────────────────────────────

/// The main D-Bus interface object for y2skk.
///
/// Bus name  : `org.y2skk.Daemon`
/// Object path: `/org/y2skk/Daemon`
pub struct DaemonInterface {
    sessions: Arc<Mutex<SessionManager>>,
    config: Arc<Mutex<Config>>,
}

impl DaemonInterface {
    pub fn new(sessions: Arc<Mutex<SessionManager>>, config: Arc<Mutex<Config>>) -> Self {
        Self { sessions, config }
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

    /// Reloads the configuration file from disk.
    async fn reload_config(&self) {
        let new_config = Config::load_default();
        *self.config.lock().await = new_config;
        tracing::info!("Config reloaded");
    }
}

// ── Daemon startup ────────────────────────────────────────────────────────────

/// Starts the D-Bus service and runs until the process receives a shutdown signal.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let sessions = Arc::new(Mutex::new(SessionManager::new(&config)));
    let config = Arc::new(Mutex::new(config));

    let interface = DaemonInterface::new(sessions.clone(), config);

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
