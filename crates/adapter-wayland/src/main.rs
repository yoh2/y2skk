mod candidate_window;
mod keymap;
mod server;

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

fn main() -> anyhow::Result<()> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Send events to both stderr (useful when run directly in a terminal) and
    // the systemd journal (useful when KWin launches us via the virtual-keyboard
    // mechanism, where stderr goes nowhere visible).  `journalctl -t y2skk-wayland`
    // shows the logs.  If journald is unreachable the layer is simply omitted.
    let journald_layer = tracing_journald::layer().ok();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(journald_layer)
        .init();

    server::run()
}
