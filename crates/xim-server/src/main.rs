//! y2skk XIM server entry point.
//!
//! Bootstraps tracing and hands control to [`server::run`] which owns the
//! blocking X11 event loop.

mod candidates;
mod handler;
mod key;
mod preedit;
mod server;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("y2skk-xim starting up");
    server::run()
}
