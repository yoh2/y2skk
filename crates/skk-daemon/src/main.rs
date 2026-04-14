use std::path::PathBuf;

use clap::Parser;
use skk_daemon::config::{self, Config};
use skk_daemon::dbus;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "y2skk IME daemon")]
struct Args {
    /// Path to config.toml (default: $XDG_CONFIG_HOME/y2skk/config.toml)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Validate config and exit without starting the daemon
    #[arg(long)]
    check_config: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let config_path = args.config.unwrap_or_else(config::default_config_path);

    let cfg = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            // Logging may not be initialised yet — print to stderr as well.
            eprintln!("y2skk-daemon: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = config::validate(&cfg) {
        eprintln!("y2skk-daemon: {}: {e}", config_path.display());
        std::process::exit(1);
    }

    if args.check_config {
        eprintln!("{}: OK", config_path.display());
        return Ok(());
    }

    // Initialise logging; RUST_LOG overrides the config value.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.daemon.log_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    dbus::run(cfg, config_path).await
}
