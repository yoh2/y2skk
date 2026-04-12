//! Build and install helper for y2skk.
//!
//! Run via `cargo xtask <subcommand> [options]`.
//!
//! Subcommands:
//!   install   [--system | --prefix <path>] [--daemon] [--gtk3] [--qt6]
//!   uninstall [--system | --prefix <path>]
//!
//! Install modes (mutually exclusive):
//!   (default)        All components to ~/.local/. No sudo required.
//!                    GTK3 and Qt6 adapters need extra env vars (see output).
//!   --system         All components to system paths. Sudo is required for
//!                    the adapter steps. Daemon goes to /usr/local/bin/.
//!   --prefix <path>  Packaging mode: all components under this prefix.
//!                    No sudo. IM module cache update is skipped.
//!                    Systemd service is NOT installed.

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

// ── CLI ───────────────────────────────────────────────────────────────────────

enum Mode {
    /// Default: everything to ~/.local/, no sudo.
    UserLocal,
    /// --system: adapters to system paths (sudo); daemon to /usr/local/bin/.
    System,
    /// --prefix <path>: packaging, everything under prefix, no sudo.
    Packaging(PathBuf),
}

impl Mode {
    fn is_packaging(&self) -> bool {
        matches!(self, Mode::Packaging(_))
    }

    fn daemon_prefix(&self) -> PathBuf {
        match self {
            Mode::UserLocal    => home_dir().join(".local"),
            Mode::System       => PathBuf::from("/usr/local"),
            Mode::Packaging(p) => p.clone(),
        }
    }
}

struct Opts {
    mode: Mode,
    daemon: bool,
    gtk3: bool,
    qt6: bool,
}

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut system = false;
        let mut prefix: Option<PathBuf> = None;
        let mut daemon = false;
        let mut gtk3 = false;
        let mut qt6 = false;
        let mut component_flag = false;

        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--system" => system = true,
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

        if system && prefix.is_some() {
            die("--system and --prefix cannot be used together");
        }

        if !component_flag {
            daemon = true;
            gtk3   = true;
            qt6    = true;
        }

        let mode = if system {
            Mode::System
        } else if let Some(p) = prefix {
            Mode::Packaging(p)
        } else {
            Mode::UserLocal
        };

        Self { mode, daemon, gtk3, qt6 }
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
            eprintln!("  cargo xtask install   [--system | --prefix <path>] [--daemon] [--gtk3] [--qt6]");
            eprintln!("  cargo xtask uninstall [--system | --prefix <path>]");
            eprintln!();
            eprintln!("INSTALL MODES (mutually exclusive):");
            eprintln!("  (default)        Install everything to ~/.local/ (no sudo required).");
            eprintln!("                   GTK3/Qt6 adapters need extra environment variables.");
            eprintln!("  --system         Install to system paths (sudo required for adapters).");
            eprintln!("                   Daemon installs to /usr/local/bin/.");
            eprintln!("  --prefix <path>  Packaging mode: install all components under <path>.");
            eprintln!("                   No sudo. IM cache update skipped. No systemd service.");
            eprintln!();
            eprintln!("COMPONENT FLAGS (default: all):");
            eprintln!("  --daemon         Daemon (+ systemd service) only");
            eprintln!("  --gtk3           GTK3 IM module only");
            eprintln!("  --qt6            Qt6 IM plugin only");
            std::process::exit(1);
        }
    }
}

// ── install ───────────────────────────────────────────────────────────────────

fn cmd_install(opts: Opts) {
    let ws = workspace_root();

    if opts.daemon {
        install_daemon(&ws, &opts.mode);
    }
    if opts.gtk3 {
        install_gtk3(&ws, &opts.mode);
    }
    if opts.qt6 {
        install_qt6(&ws, &opts.mode);
    }

    // Systemd user service: always to ~/.config/systemd/user/, skipped for packaging.
    if opts.daemon && !opts.mode.is_packaging() {
        install_systemd_service(&ws, &opts.mode.daemon_prefix());
    }

    println!();
    println!("Installation complete.");

    match &opts.mode {
        Mode::UserLocal => {
            if opts.daemon {
                println!();
                println!("To start the daemon:");
                println!("  systemctl --user enable --now y2skk-daemon");
            }
            // Remind the user which extra env vars are needed for user-local adapters.
            let need_gtk3_env = opts.gtk3 && pkg_config_exists("gtk+-3.0");
            let need_qt6_env  = opts.qt6;
            if need_gtk3_env || need_qt6_env {
                println!();
                println!("Add the following to your shell profile or session startup script:");
                if need_gtk3_env {
                    println!(r#"  export GTK_IM_MODULE_FILE="$HOME/.config/gtk-3.0/gtk.immodules""#);
                }
                if need_qt6_env {
                    println!(r#"  export QT_PLUGIN_PATH="$HOME/.local/lib/qt6/plugins:$QT_PLUGIN_PATH""#);
                }
            }
        }
        Mode::System => {
            if opts.daemon {
                println!();
                println!("To start the daemon:");
                println!("  systemctl --user enable --now y2skk-daemon");
            }
        }
        Mode::Packaging(_) => {}
    }
}

// ── daemon ─────────────────────────────────────────────────────────────────────

fn install_daemon(ws: &Path, mode: &Mode) {
    println!("==> Building y2skk-daemon (release)...");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    run(Command::new(&cargo)
        .args(["build", "--release", "-p", "skk-daemon"])
        .current_dir(ws));

    let src  = ws.join("target/release/y2skk-daemon");
    let dest = mode.daemon_prefix().join("bin/y2skk-daemon");
    let use_sudo = matches!(mode, Mode::System);
    install_file(&src, &dest, use_sudo);
}

// ── GTK3 adapter ───────────────────────────────────────────────────────────────

fn install_gtk3(ws: &Path, mode: &Mode) {
    println!("==> Building GTK3 IM module...");

    if !pkg_config_exists("gtk+-3.0") {
        println!("    [SKIP] gtk+-3.0 not found via pkg-config.");
        return;
    }

    match mode {
        Mode::UserLocal => install_gtk3_user(ws),
        Mode::System    => install_gtk3_cmake(ws, None, true),
        Mode::Packaging(prefix) => {
            let bin_ver = pkg_config_var("gtk+-3.0", "gtk_binary_version")
                .unwrap_or_else(|| "3.0.0".to_string());
            let module_dir = prefix.join("lib").join("gtk-3.0").join(&bin_ver).join("immodules");
            install_gtk3_cmake(ws, Some(&module_dir), false);
        }
    }
}

/// User-local GTK3 install: build with cargo, copy to ~/.local/, update user cache.
fn install_gtk3_user(ws: &Path) {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    run(Command::new(&cargo)
        .args(["build", "--release", "-p", "adapter-gtk3"])
        .current_dir(ws));

    let src = ws.join("target/release/libadapter_gtk3.so");
    let bin_ver = pkg_config_var("gtk+-3.0", "gtk_binary_version")
        .unwrap_or_else(|| "3.0.0".to_string());
    let dest = home_dir()
        .join(".local/lib/gtk-3.0")
        .join(&bin_ver)
        .join("immodules/im-y2skk.so");

    install_file(&src, &dest, false);
    update_gtk3_user_cache(&dest);
}

/// Update ~/.config/gtk-3.0/gtk.immodules by running gtk-query-immodules-3.0
/// with the installed .so path so GTK3 can discover the module.
fn update_gtk3_user_cache(so_path: &Path) {
    let cache_dir  = home_dir().join(".config/gtk-3.0");
    let cache_file = cache_dir.join("gtk.immodules");

    println!("    Updating GTK3 user module cache -> {}", cache_file.display());
    fs::create_dir_all(&cache_dir).ok();

    // Pass the .so path directly so gtk-query-immodules-3.0 generates an entry
    // with the full absolute path — no GTK_PATH setup needed.
    match Command::new("gtk-query-immodules-3.0").arg(so_path).output() {
        Ok(o) if o.status.success() => {
            fs::write(&cache_file, &o.stdout)
                .unwrap_or_else(|e| eprintln!("    Warning: could not write cache: {e}"));
        }
        Ok(o) => {
            eprintln!("    Warning: gtk-query-immodules-3.0 exited with {:?}", o.status.code());
        }
        Err(e) => {
            eprintln!("    Warning: gtk-query-immodules-3.0 not found: {e}");
            eprintln!("    You may need to run it manually.");
        }
    }
}

/// System or packaging GTK3 install via cmake.
/// `module_dir` — override for GTK3_IM_MODULE_DIR (None = use pkg-config default).
/// `use_sudo`   — run `cmake --install` under sudo.
fn install_gtk3_cmake(ws: &Path, module_dir: Option<&Path>, use_sudo: bool) {
    if !cmd_exists("cmake") {
        println!("    [SKIP] cmake not found.");
        return;
    }

    let build_dir = ws.join("target/xtask-build/gtk3");
    let src_dir   = ws.join("shim/gtk3");

    let module_dir_str = module_dir.map(|p| p.to_str().unwrap().to_string())
        .unwrap_or_default();
    // Explicitly pass both CACHE variables so stale cmake cache entries are overridden.
    let update_cache = if use_sudo { "ON" } else { "OFF" };

    println!("    Configuring...");
    let ok = Command::new("cmake")
        .args([
            "-S", src_dir.to_str().unwrap(),
            "-B", build_dir.to_str().unwrap(),
            "-DCMAKE_BUILD_TYPE=Release",
            &format!("-DGTK3_IM_MODULE_DIR={module_dir_str}"),
            &format!("-DGTK3_UPDATE_IMMODULE_CACHE={update_cache}"),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        println!("    [SKIP] cmake configure failed (GTK3 headers missing?).");
        return;
    }

    println!("    Building...");
    let jobs = parallelism();
    run(Command::new("cmake")
        .args(["--build", build_dir.to_str().unwrap(), "--parallel", &jobs]));

    println!("    Installing{}...", if use_sudo { " (sudo)" } else { "" });
    let install_args = ["--install", build_dir.to_str().unwrap()];
    if use_sudo {
        run_as_root(Command::new("cmake").args(install_args));
    } else {
        run(Command::new("cmake").args(install_args));
    }
}

// ── Qt6 adapter ────────────────────────────────────────────────────────────────

fn install_qt6(ws: &Path, mode: &Mode) {
    println!("==> Building Qt6 IM plugin...");

    if !cmd_exists("cmake") {
        println!("    [SKIP] cmake not found.");
        return;
    }

    let (plugin_dir, use_sudo) = match mode {
        Mode::UserLocal => {
            let dir = home_dir().join(".local/lib/qt6/plugins/platforminputcontexts");
            (Some(dir), false)
        }
        Mode::System => (None, true),
        Mode::Packaging(prefix) => {
            let dir = prefix.join("lib/qt6/plugins/platforminputcontexts");
            (Some(dir), false)
        }
    };

    install_qt6_cmake(ws, plugin_dir.as_deref(), use_sudo);
}

fn install_qt6_cmake(ws: &Path, plugin_dir: Option<&Path>, use_sudo: bool) {
    let build_dir = ws.join("target/xtask-build/qt6");
    let src_dir   = ws.join("shim/qt6");

    let plugin_dir_str = plugin_dir.map(|p| p.to_str().unwrap().to_string())
        .unwrap_or_default();

    println!("    Configuring...");
    let ok = Command::new("cmake")
        .args([
            "-S", src_dir.to_str().unwrap(),
            "-B", build_dir.to_str().unwrap(),
            "-DCMAKE_BUILD_TYPE=Release",
            &format!("-DY2SKK_QT6_PLUGIN_DIR={plugin_dir_str}"),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        println!("    [SKIP] cmake configure failed (Qt6 or Corrosion missing?).");
        return;
    }

    println!("    Building...");
    let jobs = parallelism();
    run(Command::new("cmake")
        .args(["--build", build_dir.to_str().unwrap(), "--parallel", &jobs]));

    println!("    Installing{}...", if use_sudo { " (sudo)" } else { "" });
    let install_args = ["--install", build_dir.to_str().unwrap()];
    if use_sudo {
        run_as_root(Command::new("cmake").args(install_args));
    } else {
        run(Command::new("cmake").args(install_args));
    }
}

// ── systemd service ────────────────────────────────────────────────────────────

fn install_systemd_service(ws: &Path, daemon_prefix: &Path) {
    let src = ws.join("dist/systemd/y2skk-daemon.service");
    let daemon_path = daemon_prefix.join("bin/y2skk-daemon");

    // Replace the placeholder ExecStart path with the actual installed path.
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

    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
}

// ── uninstall ─────────────────────────────────────────────────────────────────

fn cmd_uninstall(opts: Opts) {
    let mut removed = false;

    // Daemon.
    let daemon_path = opts.mode.daemon_prefix().join("bin/y2skk-daemon");
    if daemon_path.exists() {
        println!("  Removing daemon: {}", daemon_path.display());
        fs::remove_file(&daemon_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        removed = true;
    }

    // GTK3 module.
    let bin_ver = pkg_config_var("gtk+-3.0", "gtk_binary_version")
        .unwrap_or_else(|| "3.0.0".to_string());
    let gtk3_path = match &opts.mode {
        Mode::UserLocal => home_dir()
            .join(".local/lib/gtk-3.0")
            .join(&bin_ver)
            .join("immodules/im-y2skk.so"),
        Mode::System => gtk3_system_module_path(),
        Mode::Packaging(prefix) => prefix
            .join("lib/gtk-3.0")
            .join(&bin_ver)
            .join("immodules/im-y2skk.so"),
    };
    if gtk3_path.exists() {
        println!("  Removing GTK3 module: {}", gtk3_path.display());
        fs::remove_file(&gtk3_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        removed = true;

        // For user-local, also clean up the user module cache.
        if matches!(opts.mode, Mode::UserLocal) {
            let cache = home_dir().join(".config/gtk-3.0/gtk.immodules");
            if cache.exists() {
                println!("  Removing GTK3 user module cache: {}", cache.display());
                fs::remove_file(&cache).ok();
            }
        }
    }

    // Qt6 plugin.
    let qt6_path = match &opts.mode {
        Mode::UserLocal => home_dir()
            .join(".local/lib/qt6/plugins/platforminputcontexts/libqy2skk-qt6-plugin.so"),
        Mode::System => qt6_system_plugin_path(),
        Mode::Packaging(prefix) => prefix
            .join("lib/qt6/plugins/platforminputcontexts/libqy2skk-qt6-plugin.so"),
    };
    if qt6_path.exists() {
        println!("  Removing Qt6 plugin: {}", qt6_path.display());
        fs::remove_file(&qt6_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        removed = true;
    }

    // Systemd service (always user-local).
    let service = home_dir().join(".config/systemd/user/y2skk-daemon.service");
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

fn gtk3_system_module_path() -> PathBuf {
    let libdir  = pkg_config_var("gtk+-3.0", "libdir").unwrap_or_else(|| "/usr/lib".to_string());
    let bin_ver = pkg_config_var("gtk+-3.0", "gtk_binary_version")
        .unwrap_or_else(|| "3.0.0".to_string());
    PathBuf::from(libdir)
        .join("gtk-3.0").join(&bin_ver).join("immodules/im-y2skk.so")
}

fn qt6_system_plugin_path() -> PathBuf {
    let base = qmake_query("QT_INSTALL_PLUGINS")
        .unwrap_or_else(|| "/usr/lib/qt6/plugins".to_string());
    PathBuf::from(base).join("platforminputcontexts/libqy2skk-qt6-plugin.so")
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn parallelism() -> String {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .to_string()
}

/// Copy `src` to `dest`, creating parent directories as needed.
/// Uses `sudo install -D -m 755` when `use_sudo` is true.
fn install_file(src: &Path, dest: &Path, use_sudo: bool) {
    println!("    {} -> {}", src.display(), dest.display());
    if use_sudo {
        run_as_root(Command::new("install")
            .args(["-D", "-m", "755",
                   src.to_str().unwrap(),
                   dest.to_str().unwrap()]));
    } else {
        let dest_dir = dest.parent().unwrap();
        fs::create_dir_all(dest_dir)
            .unwrap_or_else(|e| die(&format!("create {}: {e}", dest_dir.display())));
        fs::copy(src, dest)
            .unwrap_or_else(|e| die(&format!("copy to {}: {e}", dest.display())));
        set_executable(dest);
    }
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
    // Try qmake6 first, then qmake as a fallback.
    for qmake in &["qmake6", "qmake"] {
        if let Ok(out) = Command::new(qmake).args(["-query", var]).output() {
            if out.status.success() {
                let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !val.is_empty() && val != "**Unknown**" {
                    return Some(val);
                }
            }
        }
    }
    None
}

fn cmd_exists(name: &str) -> bool {
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
