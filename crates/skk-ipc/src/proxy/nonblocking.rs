//! Async D-Bus client for `org.y2skk.Daemon`.
//!
//! Used by adapters that run inside a tokio runtime, notably the XIM server
//! which needs to interleave X11 I/O with D-Bus calls without dedicating one
//! OS thread per in-flight request.  The method signatures mirror
//! [`super::blocking::DaemonProxy`] so callers can pick a variant based on
//! their runtime without learning two APIs.

use zbus::Connection;
use zbus::Proxy;
use zbus::zvariant::OwnedObjectPath;

use crate::{IpcAction, SessionId};

use super::{BUS_NAME, INTERFACE, OBJECT_PATH};

/// Async proxy for `org.y2skk.Daemon`.
pub struct DaemonProxy {
    conn: Connection,
}

impl DaemonProxy {
    /// Connects to the session bus.  The daemon is contacted lazily on the
    /// first method call.
    pub async fn connect() -> zbus::Result<Self> {
        let conn = Connection::session().await?;
        Ok(Self { conn })
    }

    async fn proxy(&self) -> zbus::Result<Proxy<'_>> {
        Proxy::new(
            &self.conn,
            BUS_NAME,
            OwnedObjectPath::try_from(OBJECT_PATH).unwrap(),
            INTERFACE,
        )
        .await
    }

    /// Creates a new IME session and returns its ID.
    pub async fn create_session(&self, app_id: &str) -> zbus::Result<SessionId> {
        self.proxy().await?.call("CreateSession", &(app_id,)).await
    }

    /// Destroys an IME session, flushing any dirty user dictionary state.
    pub async fn destroy_session(&self, session_id: SessionId) -> zbus::Result<()> {
        self.proxy()
            .await?
            .call("DestroySession", &(session_id,))
            .await
    }

    /// Processes a key event and returns the actions the adapter must perform.
    ///
    /// `key_sym` and `modifiers` follow the X11 conventions documented in
    /// [`crate::keysym`] and `ipc_architecture.md`.
    pub async fn process_key(
        &self,
        session_id: SessionId,
        key_sym: u32,
        modifiers: u32,
        is_press: bool,
    ) -> zbus::Result<Vec<IpcAction>> {
        self.proxy()
            .await?
            .call("ProcessKey", &(session_id, key_sym, modifiers, is_press))
            .await
    }

    /// Asks the daemon to reload `config.toml` from disk.
    pub async fn reload_config(&self) -> zbus::Result<()> {
        self.proxy().await?.call("ReloadConfig", &()).await
    }
}
