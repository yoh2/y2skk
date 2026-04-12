//! Build and install helper for y2skk.
//!
//! Run via `cargo xtask <subcommand> [options]`.
//!
//! Subcommands:
//!   install   [--prefix <path>] [--daemon] [--gtk3] [--qt6]
//!   uninstall [--prefix <path>]
//!
//! Without --prefix:
//!   - daemon + systemd service install to ~/.local/
//!   - GTK3 / Qt6 adapters install to system paths (sudo required)
//!
//! With --prefix <path> (packaging mode):
//!   - All components install under that prefix; no sudo is used.
//!   - The IM module cache update is skipped.
//!   - The systemd service is NOT installed (it must always go to ~/.config/systemd/user/).

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ── Workspace root ─────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is set to xtask/ at compile time.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must be inside the workspace root")
        .to_path_buf()
}

fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").expect("HOME is not set"))
}

fn default_daemon_prefix() -> PathBuf {
    home_dir().join(".local")
}

// ── CLI ───────────────────────────────────────────────────────────────────────

struct Opts {
    /// Explicit --prefix given by the user (packaging mode).
    /// None  = user install: daemon → ~/.local, adapters → system (sudo)
    /// Some  = packaging: everything under this prefix, no sudo
    prefix: Option<PathBuf>,
    daemon: bool,
    gtk3: bool,
    qt6: bool,
}

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut prefix: Option<PathBuf> = None;
        let mut daemon = false;
        let mut gtk3 = false;
        let mut qt6 = false;
        let mut component_flag = false;

        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--prefix" => {
                    let val = iter.next().unwrap_or_else(|| die("--prefix requires a path"));
                    prefix = Some(PathBuf::from(val));
                }
                "--daemon" => { daemon = true; component_flag = true; }
                "--gtk3"   => { gtk3   = true; component_flag = true; }
                "--qt6"    => { qt6    = true; component_flag = true; }
                other => die(&format!("Unknown option: {other}")),
            }
        }

        if !component_flag {
            daemon = true;
            gtk3   = true;
            qt6    = true;
        }

        Self { prefix, daemon, gtk3, qt6 }
    }

    fn is_packaging(&self) -> bool {
        self.prefix.is_some()
    }

    fn daemon_prefix(&self) -> PathBuf {
        self.prefix.clone().unwrap_or_else(default_daemon_prefix)
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    let argv: Vec<String> = env::args().collect();
    match argv.get(1).map(String::as_str) {
        Some("install")   => cmd_install(Opts::parse(&argv[2..])),
        Some("uninstall") => cmd_uninstall(Opts::parse(&argv[2..])),
        _ => {
            eprintln!("y2skk build helper");
            eprintln!();
            eprintln!("USAGE:");
            eprintln!("  cargo xtask install   [--prefix <path>] [--daemon] [--gtk3] [--qt6]");
            eprintln!("  cargo xtask uninstall [--prefix <path>]");
            eprintln!();
            eprintln!("OPTIONS:");
            eprintln!("  --prefix <path>  Packaging prefix (all components under this path; no sudo).");
            eprintln!("                   Without this flag, adapters install to system paths (sudo),");
            eprintln!("                   and the daemon installs to ~/.local/.");
            eprintln!("  --daemon         Install/uninstall the daemon only");
            eprintln!("  --gtk3           Install/uninstall the GTK3 IM module only");
            eprintln!("  --qt6            Install/uninstall the Qt6 IM plugin only");
            std::process::exit(1);
        }
    }
}

// ── install ───────────────────────────────────────────────────────────────────

fn cmd_install(opts: Opts) {
    let ws = workspace_root();
    let packaging = opts.is_packaging();
    let daemon_prefix = opts.daemon_prefix();

    if opts.daemon {
        install_daemon(&ws, &daemon_prefix);
    }
    if opts.gtk3 {
        install_gtk3(&ws, opts.prefix.as_deref());
    }
    if opts.qt6 {
        install_qt6(&ws, opts.prefix.as_deref());
    }

    // Systemd user service always installs to the user directory and is only
    // relevant for user installs (not packaging).
    if opts.daemon && !packaging {
        install_systemd_service(&ws, &daemon_prefix);
    }

    println!();
    println!("Installation complete.");
    if !packaging && opts.daemon {
        println!();
        println!("To start the daemon:");
        println!("  systemctl --user enable --now y2skk-daemon");
    }
}

// ── daemon ─────────────────────────────────────────────────────────────────────

fn install_daemon(ws: &Path, prefix: &Path) {
    println!("==> Building y2skk-daemon (release)...");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    run(Command::new(&cargo)
        .args(["build", "--release", "-p", "skk-daemon"])
        .current_dir(ws));

    let src      = ws.join("target/release/y2skk-daemon");
    let dest_dir = prefix.join("bin");
    let dest     = dest_dir.join("y2skk-daemon");

    println!("    {} -> {}", src.display(), dest.display());
    fs::create_dir_all(&dest_dir)
        .unwrap_or_else(|e| die(&format!("create {}: {e}", dest_dir.display())));
    fs::copy(&src, &dest)
        .unwrap_or_else(|e| die(&format!("copy daemon: {e}")));

    set_executable(&dest);
}

// ── GTK3 adapter ───────────────────────────────────────────────────────────────

fn install_gtk3(ws: &Path, packaging_prefix: Option<&Path>) {
    println!("==> Building GTK3 IM module...");

    if !pkg_config_exists("gtk+-3.0") {
        println!("    [SKIP] gtk+-3.0 not found via pkg-config.");
        return;
    }

    if !cmd_exists("cmake") {
        println!("    [SKIP] cmake not found.");
        return;
    }

    let build_dir = ws.join("target").join("xtask-build").join("gtk3");
    let src_dir   = ws.join("shim").join("gtk3");

    // Build cmake configure args.
    let mut configure_args: Vec<String> = vec![
        "-S".to_string(), src_dir.to_str().unwrap().to_string(),
        "-B".to_string(), build_dir.to_str().unwrap().to_string(),
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
    ];

    if let Some(prefix) = packaging_prefix {
        // Packaging: override install dir and skip cache update.
        let bin_ver = pkg_config_var("gtk+-3.0", "gtk_binary_version")
            .unwrap_or_else(|| "3.0.0".to_string());
        let module_dir = prefix.join("lib").join("gtk-3.0").join(&bin_ver).join("immodules");
        configure_args.push(format!("-DGTK3_IM_MODULE_DIR={}", module_dir.display()));
        configure_args.push("-DGTK3_UPDATE_IMMODULE_CACHE=OFF".to_string());
    }

    // Configure.
    println!("    Configuring...");
    let configure_ok = Command::new("cmake")
        .args(&configure_args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !configure_ok {
        println!("    [SKIP] cmake configure failed (GTK3 headers missing?).");
        return;
    }

    // Build.
    println!("    Building...");
    let jobs = parallelism();
    run(Command::new("cmake")
        .args(["--build", build_dir.to_str().unwrap(), "--parallel", &jobs]));

    // Install.
    println!("    Installing...");
    if packaging_prefix.is_some() {
        run(Command::new("cmake")
            .args(["--install", build_dir.to_str().unwrap()]));
    } else {
        println!("    (sudo required to write to system GTK3 directory)");
        run_as_root(Command::new("cmake")
            .args(["--install", build_dir.to_str().unwrap()]));
    }
}

// ── Qt6 adapter ────────────────────────────────────────────────────────────────

fn install_qt6(ws: &Path, packaging_prefix: Option<&Path>) {
    println!("==> Building Qt6 IM plugin...");

    if !cmd_exists("cmake") {
        println!("    [SKIP] cmake not found.");
        return;
    }

    let build_dir = ws.join("target").join("xtask-build").join("qt6");
    let src_dir   = ws.join("shim").join("qt6");

    // Build cmake configure args.
    let mut configure_args: Vec<String> = vec![
        "-S".to_string(), src_dir.to_str().unwrap().to_string(),
        "-B".to_string(), build_dir.to_str().unwrap().to_string(),
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
    ];

    if let Some(prefix) = packaging_prefix {
        // Packaging: override the plugin install directory.
        let plugin_dir = prefix
            .join("lib").join("qt6").join("plugins").join("platforminputcontexts");
        configure_args.push(format!("-DY2SKK_QT6_PLUGIN_DIR={}", plugin_dir.display()));
    }

    // Configure.
    println!("    Configuring...");
    let configure_ok = Command::new("cmake")
        .args(&configure_args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !configure_ok {
        println!("    [SKIP] cmake configure failed (Qt6 or Corrosion missing?).");
        return;
    }

    // Build.
    println!("    Building...");
    let jobs = parallelism();
    run(Command::new("cmake")
        .args(["--build", build_dir.to_str().unwrap(), "--parallel", &jobs]));

    // Install.
    println!("    Installing...");
    if packaging_prefix.is_some() {
        run(Command::new("cmake")
            .args(["--install", build_dir.to_str().unwrap()]));
    } else {
        println!("    (sudo required to write to system Qt6 directory)");
        run_as_root(Command::new("cmake")
            .args(["--install", build_dir.to_str().unwrap()]));
    }
}

// ── systemd service ────────────────────────────────────────────────────────────

fn install_systemd_service(ws: &Path, prefix: &Path) {
    let src = ws.join("dist/systemd/y2skk-daemon.service");
    let daemon_path = prefix.join("bin/y2skk-daemon");

    // Replace the ExecStart line with the actual install path.
    let content = fs::read_to_string(&src)
        .unwrap_or_else(|e| die(&format!("read {}: {e}", src.display())));
    let content = content.replace(
        "%h/.cargo/bin/y2skk-daemon",
        daemon_path.to_str().unwrap(),
    );

    let dest_dir = home_dir().join(".config/systemd/user");
    let dest     = dest_dir.join("y2skk-daemon.service");

    println!("==> Installing systemd user service -> {}", dest.display());
    fs::create_dir_all(&dest_dir)
        .unwrap_or_else(|e| die(&format!("create {}: {e}", dest_dir.display())));
    fs::write(&dest, content)
        .unwrap_or_else(|e| die(&format!("write service: {e}")));

    // Reload the user daemon so the new unit is visible.
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
}

// ── uninstall ─────────────────────────────────────────────────────────────────

fn cmd_uninstall(opts: Opts) {
    let daemon_prefix = opts.daemon_prefix();
    let service = home_dir().join(".config/systemd/user/y2skk-daemon.service");
    let mut removed = false;

    let daemon_path = daemon_prefix.join("bin/y2skk-daemon");
    if daemon_path.exists() {
        println!("  Removing daemon: {}", daemon_path.display());
        fs::remove_file(&daemon_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        removed = true;
    }

    // GTK3 module path.
    let gtk3_path = if let Some(prefix) = &opts.prefix {
        let bin_ver = pkg_config_var("gtk+-3.0", "gtk_binary_version")
            .unwrap_or_else(|| "3.0.0".to_string());
        prefix.join("lib").join("gtk-3.0").join(&bin_ver).join("immodules").join("im-y2skk.so")
    } else {
        gtk3_system_module_path()
    };
    if gtk3_path.exists() {
        println!("  Removing GTK3 module: {}", gtk3_path.display());
        fs::remove_file(&gtk3_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        removed = true;
    }

    // Qt6 plugin path.
    let qt6_path = if let Some(prefix) = &opts.prefix {
        prefix.join("lib").join("qt6").join("plugins").join("platforminputcontexts")
              .join("libqy2skk-qt6-plugin.so")
    } else {
        qt6_system_plugin_path()
    };
    if let Some(ref p) = qt6_path.to_str().map(|_| qt6_path.clone()) {
        if p.exists() {
            println!("  Removing Qt6 plugin: {}", p.display());
            fs::remove_file(p).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
            removed = true;
        }
    }

    if service.exists() {
        println!("  Removing systemd service: {}", service.display());
        fs::remove_file(&service).ok();
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
        removed = true;
    }

    if removed {
        println!("Uninstall complete.");
    } else {
        println!("Nothing to uninstall.");
    }
}

/// Returns the system GTK3 IM module path from pkg-config.
fn gtk3_system_module_path() -> PathBuf {
    let libdir  = pkg_config_var("gtk+-3.0", "libdir").unwrap_or_else(|| "/usr/lib".to_string());
    let bin_ver = pkg_config_var("gtk+-3.0", "gtk_binary_version")
        .unwrap_or_else(|| "3.0.0".to_string());
    PathBuf::from(libdir)
        .join("gtk-3.0")
        .join(&bin_ver)
        .join("immodules")
        .join("im-y2skk.so")
}

/// Returns the system Qt6 plugin path from qmake.
fn qt6_system_plugin_path() -> PathBuf {
    let base = qmake_query("QT_INSTALL_PLUGINS")
        .unwrap_or_else(|| "/usr/lib/qt6/plugins".to_string());
    PathBuf::from(base)
        .join("platforminputcontexts")
        .join("libqy2skk-qt6-plugin.so")
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn parallelism() -> String {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .to_string()
}

fn run(cmd: &mut Command) {
    let prog = cmd.get_program().to_string_lossy().to_string();
    let status = cmd.status()
        .unwrap_or_else(|e| die(&format!("failed to run {prog}: {e}")));
    if !status.success() {
        die(&format!("{prog} exited with {status}"));
    }
}

/// Re-run a command under sudo by prepending `sudo` to the program and args.
fn run_as_root(cmd: &mut Command) {
    let prog: OsString = cmd.get_program().to_os_string();
    let args: Vec<OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
    let mut sudo_cmd = Command::new("sudo");
    sudo_cmd.arg(prog);
    sudo_cmd.args(args);
    run(&mut sudo_cmd);
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn pkg_config_exists(pkg: &str) -> bool {
    Command::new("pkg-config")
        .args(["--exists", pkg])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn pkg_config_var(pkg: &str, var: &str) -> Option<String> {
    let out = Command::new("pkg-config")
        .args(["--variable", var, pkg])
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn qmake_query(var: &str) -> Option<String> {
    // Try qmake6 first (Gentoo), then qmake as a fallback.
    for qmake in &["qmake6", "qmake"] {
        let out = Command::new(qmake)
            .args(["-query", var])
            .output()
            .ok()?;
        if out.status.success() {
            let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !val.is_empty() && val != "**Unknown**" {
                return Some(val);
            }
        }
    }
    None
}

fn cmd_exists(name: &str) -> bool {
    // Try spawning with --version; a NotFound error means it's absent.
    Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).ok();
    }
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) {}
