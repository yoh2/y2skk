//! Blocking D-Bus client for `org.y2skk.Daemon`.
//!
//! Used by synchronous adapter runtimes (GTK3 IM module, Qt6 platform input
//! context) that invoke the daemon from inside toolkit callbacks without a
//! surrounding async runtime.

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

use crate::{IpcAction, SessionId};

use super::{BUS_NAME, INTERFACE, OBJECT_PATH};

/// Blocking proxy for `org.y2skk.Daemon`.
pub struct DaemonProxy {
    conn: Connection,
}

impl DaemonProxy {
    /// Connects to the session bus.  The daemon itself is only contacted lazily
    /// on the first method call.
    pub fn connect() -> zbus::Result<Self> {
        let conn = Connection::session()?;
        Ok(Self { conn })
    }

    fn proxy(&self) -> zbus::Result<Proxy<'_>> {
        Proxy::new(
            &self.conn,
            BUS_NAME,
            OwnedObjectPath::try_from(OBJECT_PATH).unwrap(),
            INTERFACE,
        )
    }

    /// Creates a new IME session and returns its ID.
    pub fn create_session(&self, app_id: &str) -> zbus::Result<SessionId> {
        self.proxy()?.call("CreateSession", &(app_id,))
    }

    /// Destroys an IME session, flushing any dirty user dictionary state.
    pub fn destroy_session(&self, session_id: SessionId) -> zbus::Result<()> {
        self.proxy()?.call("DestroySession", &(session_id,))
    }

    /// Processes a key event and returns the actions the adapter must perform.
    ///
    /// `key_sym` and `modifiers` follow the X11 conventions documented in
    /// [`crate::keysym`] and `ipc_architecture.md`.
    pub fn process_key(
        &self,
        session_id: SessionId,
        key_sym: u32,
        modifiers: u32,
        is_press: bool,
    ) -> zbus::Result<Vec<IpcAction>> {
        self.proxy()?
            .call("ProcessKey", &(session_id, key_sym, modifiers, is_press))
    }

    /// Asks the daemon to reload `config.toml` from disk.
    pub fn reload_config(&self) -> zbus::Result<()> {
        self.proxy()?.call("ReloadConfig", &())
    }
}
