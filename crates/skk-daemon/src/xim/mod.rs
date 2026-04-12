//! Integrated XIM server.
//!
//! Runs inside the daemon process so key events go directly to
//! `SessionManager` without a D-Bus round-trip.  The blocking X11 event
//! loop is executed on a dedicated thread via `tokio::task::spawn_blocking`.

mod candidates;
mod handler;
mod key;
mod preedit;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::info;
use x11rb::connection::Connection;
use x11rb::rust_connection::RustConnection;
use xim::x11rb::X11rbServer;
use xim::XimConnections;

use crate::session::SessionManager;

use self::candidates::CandidateWindow;
use self::handler::{Handler, IcData};
use self::key::KeyMap;
use self::preedit::PreeditWindow;

/// Server name advertised via `XIM_SERVERS`.  Clients connect with
/// `XMODIFIERS=@im=y2skk`.
const IM_NAME: &str = "y2skk";

/// Locales advertised to XIM clients.
const LOCALES: &str = "ja_JP.UTF-8,ja_JP.eucJP,ja_JP,en_US.UTF-8,C";

/// Poll interval when the X event queue is empty (50 ms).
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Spawns the XIM server on a blocking thread.
///
/// The server shares the daemon's `SessionManager` and processes key events
/// in-process.  Returns a shutdown handle and a `JoinHandle`.  Call
/// `shutdown.store(true, Ordering::Relaxed)` to ask the event loop to exit.
pub fn spawn(
    sessions: Arc<Mutex<SessionManager>>,
) -> (Arc<AtomicBool>, tokio::task::JoinHandle<Result<()>>) {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let handle = tokio::task::spawn_blocking(move || run(sessions, shutdown_clone));
    (shutdown, handle)
}

/// Blocking XIM server entry point.
fn run(sessions: Arc<Mutex<SessionManager>>, shutdown: Arc<AtomicBool>) -> Result<()> {
    // Connect to the X server.
    let (conn, screen_num) =
        RustConnection::connect(None).context("failed to connect to X server")?;
    let conn = Arc::new(conn);
    info!("XIM: connected to X server, screen {screen_num}");

    // Fetch keyboard mapping for keycode → keysym conversion.
    let keymap =
        KeyMap::new(conn.as_ref(), conn.setup()).context("failed to fetch keyboard mapping")?;
    info!("XIM: keyboard mapping loaded");

    // Create preedit window.
    let screen = &conn.setup().roots[screen_num];
    let preedit = PreeditWindow::new(Arc::clone(&conn), screen)
        .context("failed to create preedit window")?;
    info!("XIM: preedit window ready");

    // Initialise XIM server — creates the server window, acquires the
    // XIM_SERVERS selection, and advertises locales + transport.
    let mut server: X11rbServer<Arc<RustConnection>> =
        X11rbServer::init(Arc::clone(&conn), screen_num, IM_NAME, LOCALES)
            .map_err(|e| anyhow::anyhow!("XIM server init failed: {e}"))?;
    info!("XIM: registered as @server={IM_NAME}");

    let mut connections: XimConnections<IcData> = XimConnections::new();
    let candidates = CandidateWindow::new(Arc::clone(&conn), screen)
        .context("failed to create candidate window")?;
    info!("XIM: candidate window ready");

    let mut handler = Handler::new(sessions, keymap, preedit, candidates);

    // Event loop using poll_for_event so we can check the shutdown flag.
    loop {
        if shutdown.load(Ordering::Relaxed) {
            info!("XIM: shutdown requested, exiting event loop");
            break;
        }

        match conn.poll_for_event()? {
            Some(event) => {
                if let Err(e) = server.filter_event(&event, &mut connections, &mut handler) {
                    tracing::warn!("XIM: filter_event error: {e}");
                }
            }
            None => {
                // No events queued — sleep briefly before polling again.
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }

    Ok(())
}
