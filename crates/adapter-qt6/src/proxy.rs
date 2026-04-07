use skk_ipc::{IpcAction, SessionId};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

/// Blocking D-Bus proxy for `org.y2skk.Daemon`.
///
/// Uses `zbus::blocking` so key processing can be done synchronously
/// from within Qt's `filterEvent` without spinning an async runtime.
pub struct DaemonProxy {
    conn: Connection,
}

impl DaemonProxy {
    const BUS_NAME: &'static str = "org.y2skk.Daemon";
    const OBJECT_PATH: &'static str = "/org/y2skk/Daemon";
    const INTERFACE: &'static str = "org.y2skk.Daemon";

    pub fn connect() -> zbus::Result<Self> {
        let conn = Connection::session()?;
        Ok(Self { conn })
    }

    fn proxy(&self) -> zbus::Result<Proxy<'_>> {
        Proxy::new(
            &self.conn,
            Self::BUS_NAME,
            OwnedObjectPath::try_from(Self::OBJECT_PATH).unwrap(),
            Self::INTERFACE,
        )
    }

    pub fn create_session(&self, app_id: &str) -> zbus::Result<SessionId> {
        self.proxy()?.call("CreateSession", &(app_id,))
    }

    pub fn destroy_session(&self, session_id: SessionId) -> zbus::Result<()> {
        self.proxy()?.call("DestroySession", &(session_id,))
    }

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
}
