//! XIM server bootstrap and event loop.
//!
//! Sets up the X11 connection, registers as `@server=y2skk`, fetches the
//! keyboard mapping, connects to skk-daemon, and enters the blocking X11
//! event loop.

use anyhow::{Context, Result};
use tracing::info;
use x11rb::connection::Connection;
use x11rb::rust_connection::RustConnection;
use xim::x11rb::X11rbServer;
use xim::XimConnections;

use skk_ipc::proxy::blocking::DaemonProxy;

use crate::handler::{Handler, IcData};
use crate::key::KeyMap;

/// Server name advertised via `XIM_SERVERS`.  Clients connect with
/// `XMODIFIERS=@im=y2skk`.
const IM_NAME: &str = "y2skk";

/// Locales advertised to XIM clients.
const LOCALES: &str = "ja_JP.UTF-8,ja_JP.eucJP,ja_JP,en_US.UTF-8,C";

/// Runs the XIM server.  This function blocks until an error or signal.
pub fn run() -> Result<()> {
    // Connect to the X server.
    let (conn, screen_num) =
        RustConnection::connect(None).context("failed to connect to X server")?;
    info!("connected to X server, screen {screen_num}");

    // Fetch keyboard mapping for keycode → keysym conversion.
    let keymap =
        KeyMap::new(&conn, conn.setup()).context("failed to fetch keyboard mapping")?;
    info!("keyboard mapping loaded");

    // Connect to skk-daemon via D-Bus (blocking).
    let proxy = DaemonProxy::connect().context("failed to connect to skk-daemon")?;
    info!("connected to skk-daemon");

    // Initialise XIM server — this creates the server window, acquires the
    // XIM_SERVERS selection, and advertises locales + transport.
    let mut server: X11rbServer<&RustConnection> =
        X11rbServer::init(&conn, screen_num, IM_NAME, LOCALES)
            .map_err(|e| anyhow::anyhow!("XIM server init failed: {e}"))?;
    info!("XIM server registered as @server={IM_NAME}");

    let mut connections: XimConnections<IcData> = XimConnections::new();
    let mut handler = Handler::new(proxy, keymap);

    // Blocking event loop — runs until the X connection drops or we get a
    // fatal error.  Ctrl-C is handled by the tokio runtime in main.rs.
    loop {
        let event = conn
            .wait_for_event()
            .context("X11 wait_for_event failed")?;
        if let Err(e) = server.filter_event(&event, &mut connections, &mut handler) {
            tracing::warn!("XIM filter_event error: {e}");
        }
    }
}
