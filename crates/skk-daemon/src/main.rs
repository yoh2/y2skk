use skk_daemon::config::Config;
use skk_daemon::dbus;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load_default();

    // Initialise logging; RUST_LOG overrides the config value
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.daemon.log_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    dbus::run(config).await
}
