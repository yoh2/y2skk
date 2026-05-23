//! Build and install helper for y2skk.
//!
//! Run via `cargo xtask <subcommand> [options]`.
//!
//! Subcommands:
//!   install   [--system | --prefix <path>] [--daemon] [--xim] [--gtk3] [--gtk4] [--qt6] [--wayland] [--gui]
//!   uninstall [--system | --prefix <path>]
//!
//! Install modes (mutually exclusive):
//!   (default)        All components to ~/.local/. No sudo required.
//!                    GTK3 and Qt6 adapters need extra env vars (see output).
//!   --system         All components to system paths. Sudo is required for
//!                    the adapter steps. Daemon goes to /usr/local/bin/.
//!   --prefix <path>  Packaging mode: all components under this prefix.
//!                    No sudo. IM module cache update is skipped.
//!                    Systemd services are NOT installed.

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
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| die("HOME is not set"))
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
            Mode::UserLocal => home_dir().join(".local"),
            Mode::System => PathBuf::from("/usr/local"),
            Mode::Packaging(p) => p.clone(),
        }
    }
}

struct Opts {
    mode: Mode,
    daemon: bool,
    xim: bool,
    gtk3: bool,
    gtk4: bool,
    qt6: bool,
    wayland: bool,
    gui: bool,
    /// Opt-in: run `systemctl --user try-restart` for installed services.
    /// Always ignored in Packaging mode (must not touch user environment).
    restart: bool,
}

impl Opts {
    fn parse(args: &[String]) -> Self {
        let mut system = false;
        let mut prefix: Option<PathBuf> = None;
        let mut daemon = false;
        let mut xim = false;
        let mut gtk3 = false;
        let mut gtk4 = false;
        let mut qt6 = false;
        let mut wayland = false;
        let mut gui = false;
        let mut restart = false;
        let mut component_flag = false;

        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--system" => system = true,
                "--prefix" => {
                    let val = iter
                        .next()
                        .unwrap_or_else(|| die("--prefix requires a path"));
                    prefix = Some(PathBuf::from(val));
                }
                "--daemon" => {
                    daemon = true;
                    component_flag = true;
                }
                "--xim" => {
                    xim = true;
                    component_flag = true;
                }
                "--gtk3" => {
                    gtk3 = true;
                    component_flag = true;
                }
                "--gtk4" => {
                    gtk4 = true;
                    component_flag = true;
                }
                "--qt6" => {
                    qt6 = true;
                    component_flag = true;
                }
                "--wayland" => {
                    wayland = true;
                    component_flag = true;
                }
                "--gui" => {
                    gui = true;
                    component_flag = true;
                }
                "--restart" => restart = true,
                other => die(&format!("Unknown option: {other}")),
            }
        }

        if system && prefix.is_some() {
            die("--system and --prefix cannot be used together");
        }

        if !component_flag {
            daemon = true;
            xim = true;
            gtk3 = true;
            gtk4 = true;
            qt6 = true;
            wayland = true;
            gui = true;
        }

        let mode = if system {
            Mode::System
        } else if let Some(p) = prefix {
            Mode::Packaging(p)
        } else {
            Mode::UserLocal
        };

        Self {
            mode,
            daemon,
            xim,
            gtk3,
            gtk4,
            qt6,
            wayland,
            gui,
            restart,
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    let argv: Vec<String> = env::args().collect();
    match argv.get(1).map(String::as_str) {
        Some("install") => cmd_install(Opts::parse(&argv[2..])),
        Some("uninstall") => cmd_uninstall(Opts::parse(&argv[2..])),
        _ => {
            eprintln!("y2skk build helper");
            eprintln!();
            eprintln!("USAGE:");
            eprintln!("  cargo xtask install   [--system | --prefix <path>] [--daemon] [--xim] [--gtk3] [--gtk4] [--qt6] [--wayland] [--gui]");
            eprintln!("  cargo xtask uninstall [--system | --prefix <path>]");
            eprintln!();
            eprintln!("INSTALL MODES (mutually exclusive):");
            eprintln!("  (default)        Install everything to ~/.local/ (no sudo required).");
            eprintln!("                   GTK3/Qt6 adapters need extra environment variables.");
            eprintln!("  --system         Install to system paths (sudo required for adapters).");
            eprintln!("                   Daemon and XIM server install to /usr/local/bin/.");
            eprintln!("  --prefix <path>  Packaging mode: install all components under <path>.");
            eprintln!("                   No sudo. IM cache update skipped. No systemd services.");
            eprintln!();
            eprintln!("COMPONENT FLAGS (default: all):");
            eprintln!("  --daemon         Daemon (+ systemd service) only");
            eprintln!("  --xim            XIM server (+ systemd service) only");
            eprintln!("  --gtk3           GTK3 IM module only");
            eprintln!("  --gtk4           GTK4 IM module only");
            eprintln!("  --qt6            Qt6 IM plugin only");
            eprintln!("  --gui            Qt6 settings GUI (y2skk-settings + launcher) only");
            eprintln!("  --wayland        Wayland adapter (+ KDE virtual-keyboard entry) only.");
            eprintln!("                   Always kills the running y2skk-wayland after install");
            eprintln!("                   (outside packaging mode) so KWin relaunches the new");
            eprintln!("                   binary on the next text-input focus.");
            eprintln!();
            eprintln!("OTHER:");
            eprintln!("  --restart        After install, run `systemctl --user try-restart`");
            eprintln!("                   for installed services. Off by default. Ignored in");
            eprintln!("                   --prefix (packaging) mode so user state is untouched.");
            std::process::exit(1);
        }
    }
}

// ── install ───────────────────────────────────────────────────────────────────

fn cmd_install(opts: Opts) {
    let ws = workspace_root();

    if opts.daemon {
        install_daemon(&ws, &opts.mode);
        install_tables(&ws, &opts.mode);
    }
    if opts.xim {
        install_xim(&ws, &opts.mode);
    }
    if opts.gtk3 {
        install_gtk3(&ws, &opts.mode);
    }
    if opts.gtk4 {
        install_gtk4(&ws, &opts.mode);
    }
    if opts.qt6 {
        install_qt6(&ws, &opts.mode);
    }
    if opts.wayland {
        install_wayland(&ws, &opts.mode);
    }
    // Only install the launcher if the GUI binary was actually built/installed.
    let gui_installed = opts.gui && install_gui(&ws, &opts.mode);

    // Systemd user services and D-Bus activation: skipped for packaging.
    if !opts.mode.is_packaging() {
        let prefix = opts.mode.daemon_prefix();
        if opts.daemon {
            install_daemon_service(&ws, &prefix);
            install_dbus_activation(&ws, &prefix);
        }
        if opts.xim {
            install_xim_service(&ws, &prefix);
        }
        if opts.wayland {
            install_wayland_desktop(&ws, &prefix);
        }
        if gui_installed {
            install_gui_desktop(&ws, &prefix);
        }
    }

    // Optional service restart (opt-in, never in packaging mode).
    // Systemd-managed services (daemon, xim) are only touched with --restart.
    let did_restart = if opts.restart && !opts.mode.is_packaging() {
        let mut any = false;
        if opts.daemon {
            systemctl_try_restart("y2skk-daemon");
            any = true;
        }
        if opts.xim {
            systemctl_try_restart("y2skk-xim");
            any = true;
        }
        any
    } else {
        false
    };

    // y2skk-wayland is launched on demand by KWin's virtual-keyboard
    // mechanism, so it has no systemd unit. After a fresh binary lands,
    // any process that is still running is using the old code, so kill it
    // unconditionally — KWin will relaunch with the new binary the next
    // time a text input activates. Skipped in packaging mode (which must
    // not touch the running user environment) and treated as a success if
    // no process was running.
    if opts.wayland && !opts.mode.is_packaging() {
        pkill_wayland();
    }

    println!();
    println!("Installation complete.");

    match &opts.mode {
        Mode::UserLocal | Mode::System => {
            if (opts.daemon || opts.xim) && !did_restart {
                println!();
                println!("First-time setup (enable & start the service):");
                if opts.daemon {
                    println!("  systemctl --user enable --now y2skk-daemon");
                }
                if opts.xim {
                    println!("  systemctl --user enable --now y2skk-xim");
                }
                println!();
                println!("Already enabled?  Apply this update by restarting:");
                let mut units = String::new();
                if opts.daemon {
                    units.push_str("y2skk-daemon ");
                }
                if opts.xim {
                    units.push_str("y2skk-xim ");
                }
                println!("  systemctl --user try-restart {}", units.trim_end());
                println!("  (or re-run `cargo xtask install` with --restart)");
            }
            if matches!(opts.mode, Mode::UserLocal) {
                // Remind the user which extra env vars are needed for user-local adapters.
                let need_gtk3_env = opts.gtk3 && pkg_config_exists("gtk+-3.0");
                let need_gtk4_env = opts.gtk4 && pkg_config_exists("gtk4");
                let need_qt6_env = opts.qt6;
                if need_gtk3_env || need_gtk4_env || need_qt6_env {
                    println!();
                    println!("Add the following to your shell profile or session startup script:");
                    if need_gtk3_env {
                        println!(
                            r#"  export GTK_IM_MODULE_FILE="$HOME/.config/gtk-3.0/gtk.immodules""#
                        );
                    }
                    if need_gtk4_env {
                        // GIO does not scan ~/.local/lib/gtk-4.0/immodules/ by
                        // default; this env var tells it to. The cache file
                        // is regenerated for us by gio-querymodules during
                        // install.
                        println!(
                            r#"  export GIO_EXTRA_MODULES="$HOME/.local/lib/gtk-4.0/immodules:$GIO_EXTRA_MODULES""#
                        );
                    }
                    if need_qt6_env {
                        println!(
                            r#"  export QT_PLUGIN_PATH="$HOME/.local/lib/qt6/plugins:$QT_PLUGIN_PATH""#
                        );
                    }
                }
            }
        }
        Mode::Packaging(_) => {
            if opts.restart {
                println!();
                println!("Note: --restart is ignored in packaging (--prefix) mode.");
            }
        }
    }
}

/// Restart a user systemd unit if it is loaded and enabled.  No-op otherwise,
/// so it is safe to call on first install where the unit was just created but
/// the user has not yet `enable`d it.
fn systemctl_try_restart(unit: &str) {
    println!("==> systemctl --user try-restart {unit}");
    let _ = Command::new("systemctl")
        .args(["--user", "try-restart", unit])
        .status();
}

/// Kill any running `y2skk-wayland` so KWin relaunches with the freshly
/// installed binary. `pkill` exits with status 1 when no process matched;
/// we treat that as success (just means there was nothing to kill). Any
/// other failure (pkill missing, syntax error, permission denied, signal
/// abort) is surfaced as a warning so the user knows an old adapter may
/// still be running.
fn pkill_wayland() {
    println!("==> pkill -x y2skk-wayland (kwin will relaunch with the new binary)");
    match Command::new("pkill").args(["-x", "y2skk-wayland"]).status() {
        Ok(status) => match status.code() {
            Some(0) | Some(1) => {} // 0 = matched; 1 = nothing to kill
            Some(code) => {
                eprintln!(
                    "warning: pkill exited with status {code}; old y2skk-wayland may still be running"
                );
            }
            None => {
                eprintln!(
                    "warning: pkill terminated by signal; old y2skk-wayland may still be running"
                );
            }
        },
        Err(e) => {
            eprintln!(
                "warning: failed to spawn pkill ({e}); old y2skk-wayland may still be running"
            );
        }
    }
}

// ── daemon ─────────────────────────────────────────────────────────────────────

fn install_daemon(ws: &Path, mode: &Mode) {
    println!("==> Building y2skk-daemon (release)...");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    run(Command::new(&cargo)
        .args(["build", "--release", "-p", "skk-daemon"])
        .current_dir(ws));

    let src = ws.join("target/release/y2skk-daemon");
    let dest = mode.daemon_prefix().join("bin/y2skk-daemon");
    let use_sudo = matches!(mode, Mode::System);
    install_file(&src, &dest, use_sudo);
}

// ── kana tables ───────────────────────────────────────────────────────────────

fn install_tables(ws: &Path, mode: &Mode) {
    let src_dir = ws.join("dist/tables");
    let dest_dir = mode.daemon_prefix().join("share/y2skk/tables");
    let use_sudo = matches!(mode, Mode::System);

    println!("==> Installing kana tables -> {}", dest_dir.display());
    fs::create_dir_all(&dest_dir).unwrap_or_else(|e| {
        if use_sudo {
            run(Command::new("sudo").args(["mkdir", "-p"]).arg(&dest_dir));
        } else {
            panic!("failed to create {}: {e}", dest_dir.display());
        }
    });

    for entry in
        fs::read_dir(&src_dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", src_dir.display()))
    {
        let entry = entry.unwrap();
        let src = entry.path();
        if src.extension().and_then(|e| e.to_str()) == Some("txt") {
            let dest = dest_dir.join(entry.file_name());
            install_file(&src, &dest, use_sudo);
        }
    }
}

// ── XIM server ────────────────────────────────────────────────────────────────

fn install_xim(ws: &Path, mode: &Mode) {
    println!("==> Building y2skk-xim (release)...");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    run(Command::new(&cargo)
        .args(["build", "--release", "-p", "xim-server"])
        .current_dir(ws));

    let src = ws.join("target/release/y2skk-xim");
    let dest = mode.daemon_prefix().join("bin/y2skk-xim");
    let use_sudo = matches!(mode, Mode::System);
    install_file(&src, &dest, use_sudo);
}

// ── Wayland adapter ───────────────────────────────────────────────────────────

fn install_wayland(ws: &Path, mode: &Mode) {
    println!("==> Building y2skk-wayland (release)...");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    run(Command::new(&cargo)
        .args(["build", "--release", "-p", "adapter-wayland"])
        .current_dir(ws));

    let src = ws.join("target/release/y2skk-wayland");
    let dest = mode.daemon_prefix().join("bin/y2skk-wayland");
    let use_sudo = matches!(mode, Mode::System);
    install_file(&src, &dest, use_sudo);
}

/// Install the KDE virtual-keyboard registration (`.desktop` for System
/// Settings → Virtual Keyboard).
fn install_wayland_desktop(ws: &Path, prefix: &Path) {
    let bin_path = prefix.join("bin/y2skk-wayland");

    let app_src = ws.join("dist/applications/y2skk-wayland.desktop");
    let app_content = fs::read_to_string(&app_src)
        .unwrap_or_else(|e| die(&format!("read {}: {e}", app_src.display())));
    let app_content = app_content.replace("%Y2SKK_WAYLAND_BIN%", bin_path.to_str().unwrap());
    let app_dest = home_dir().join(".local/share/applications/y2skk-wayland.desktop");
    write_user_file(&app_dest, &app_content);
    println!("  Installed virtual-keyboard entry: {}", app_dest.display());

    println!();
    println!("To finish enabling the Wayland adapter:");
    println!("  System Settings → Keyboard → Virtual Keyboard → select \"y2skk\"");
    println!("  (KDE remembers the choice across sessions)");
}

/// Write `content` to `dest`, creating parent directories as needed.
/// Used for per-user config files — never needs sudo.
fn write_user_file(dest: &Path, content: &str) {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|e| die(&format!("create dir {}: {e}", parent.display())));
    }
    fs::write(dest, content).unwrap_or_else(|e| die(&format!("write {}: {e}", dest.display())));
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
        Mode::System => install_gtk3_cmake(ws, None, true),
        Mode::Packaging(prefix) => {
            let bin_ver = pkg_config_var("gtk+-3.0", "gtk_binary_version")
                .unwrap_or_else(|| "3.0.0".to_string());
            let module_dir = prefix
                .join("lib")
                .join("gtk-3.0")
                .join(&bin_ver)
                .join("immodules");
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
    let bin_ver =
        pkg_config_var("gtk+-3.0", "gtk_binary_version").unwrap_or_else(|| "3.0.0".to_string());
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
    let cache_dir = home_dir().join(".config/gtk-3.0");
    let cache_file = cache_dir.join("gtk.immodules");

    println!(
        "    Updating GTK3 user module cache -> {}",
        cache_file.display()
    );
    fs::create_dir_all(&cache_dir).ok();

    // Pass the .so path directly so gtk-query-immodules-3.0 generates an entry
    // with the full absolute path — no GTK_PATH setup needed.
    match Command::new("gtk-query-immodules-3.0")
        .arg(so_path)
        .output()
    {
        Ok(o) if o.status.success() => {
            fs::write(&cache_file, &o.stdout)
                .unwrap_or_else(|e| eprintln!("    Warning: could not write cache: {e}"));
        }
        Ok(o) => {
            eprintln!(
                "    Warning: gtk-query-immodules-3.0 exited with {:?}",
                o.status.code()
            );
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
    let src_dir = ws.join("shim/gtk3");

    // Explicitly pass GTK3_UPDATE_IMMODULE_CACHE so stale cmake cache entries are
    // overridden.  Only pass GTK3_IM_MODULE_DIR when a non-default path is needed;
    // passing an empty string would override the pkg-config-derived default with "".
    let update_cache = if use_sudo { "ON" } else { "OFF" };

    let mut cmake_args: Vec<String> = vec![
        "-S".into(),
        src_dir.to_str().unwrap().into(),
        "-B".into(),
        build_dir.to_str().unwrap().into(),
        "-DCMAKE_BUILD_TYPE=Release".into(),
        format!("-DGTK3_UPDATE_IMMODULE_CACHE={update_cache}"),
    ];
    if let Some(dir) = module_dir {
        cmake_args.push(format!("-DGTK3_IM_MODULE_DIR={}", dir.to_str().unwrap()));
    }

    println!("    Configuring...");
    let ok = Command::new("cmake")
        .args(&cmake_args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        println!("    [SKIP] cmake configure failed (GTK3 headers missing?).");
        return;
    }

    println!("    Building...");
    let jobs = parallelism();
    run(Command::new("cmake").args(["--build", build_dir.to_str().unwrap(), "--parallel", &jobs]));

    println!("    Installing{}...", if use_sudo { " (sudo)" } else { "" });
    let install_args = ["--install", build_dir.to_str().unwrap()];
    if use_sudo {
        run_as_root(Command::new("cmake").args(install_args));
    } else {
        run(Command::new("cmake").args(install_args));
    }
}

// ── GTK4 adapter ──────────────────────────────────────────────────────────────

fn install_gtk4(ws: &Path, mode: &Mode) {
    println!("==> Building GTK4 IM module...");

    if !pkg_config_exists("gtk4") {
        println!("    [SKIP] gtk4 not found via pkg-config.");
        return;
    }

    match mode {
        Mode::UserLocal => install_gtk4_user(ws),
        Mode::System => install_gtk4_cmake(ws, None, true),
        Mode::Packaging(prefix) => {
            // GTK4 immodules live in a flat $LIBDIR/gtk-4.0/immodules/ directory
            // (no per-binary-version subdirectory like GTK3).
            let module_dir = prefix.join("lib").join("gtk-4.0").join("immodules");
            install_gtk4_cmake(ws, Some(&module_dir), false);
        }
    }
}

/// User-local GTK4 install: build with cargo, copy to ~/.local/, and
/// (re)generate the GIO module cache for that directory so GIO can find
/// the module without a full directory rescan.
fn install_gtk4_user(ws: &Path) {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    run(Command::new(&cargo)
        .args(["build", "--release", "-p", "adapter-gtk4"])
        .current_dir(ws));

    let src = ws.join("target/release/libadapter_gtk4.so");
    // GIO requires module file names to start with "lib".
    let module_dir = home_dir().join(".local/lib/gtk-4.0/immodules");
    let dest = module_dir.join("libim-y2skk.so");

    install_file(&src, &dest, false);
    update_gio_module_cache(&module_dir);
}

/// Run `gio-querymodules <dir>` so GIO updates `<dir>/giomodule.cache`.
/// Without this (or `GIO_MODULE_DIR` cache regeneration by the distro
/// package manager) GIO will not pick up modules dropped into a
/// non-default directory like `~/.local/lib/gtk-4.0/immodules/`.
fn update_gio_module_cache(module_dir: &Path) {
    println!(
        "    Updating GIO module cache -> {}/giomodule.cache",
        module_dir.display()
    );
    match Command::new("gio-querymodules").arg(module_dir).status() {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("    Warning: gio-querymodules exited with {:?}", s.code()),
        Err(e) => eprintln!(
            "    Warning: gio-querymodules not found: {e}\n    \
             Install glib2 utilities (or run gio-querymodules manually) so GTK4 \
             can discover the module."
        ),
    }
}

/// System or packaging GTK4 install via cmake.
fn install_gtk4_cmake(ws: &Path, module_dir: Option<&Path>, use_sudo: bool) {
    if !cmd_exists("cmake") {
        println!("    [SKIP] cmake not found.");
        return;
    }

    let build_dir = ws.join("target/xtask-build/gtk4");
    let src_dir = ws.join("shim/gtk4");

    // Refresh the GIO module cache when installing into a real system path
    // (use_sudo=true). Skipped for packaging mode (use_sudo=false), which
    // installs into a staging directory the host system never sees.
    let update_cache = if use_sudo { "ON" } else { "OFF" };

    let mut cmake_args: Vec<String> = vec![
        "-S".into(),
        src_dir.to_str().unwrap().into(),
        "-B".into(),
        build_dir.to_str().unwrap().into(),
        "-DCMAKE_BUILD_TYPE=Release".into(),
        format!("-DGTK4_UPDATE_GIOMODULE_CACHE={update_cache}"),
    ];
    if let Some(dir) = module_dir {
        cmake_args.push(format!("-DGTK4_IM_MODULE_DIR={}", dir.to_str().unwrap()));
    }

    println!("    Configuring...");
    let ok = Command::new("cmake")
        .args(&cmake_args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        println!("    [SKIP] cmake configure failed (GTK4 headers missing?).");
        return;
    }

    println!("    Building...");
    let jobs = parallelism();
    run(Command::new("cmake").args(["--build", build_dir.to_str().unwrap(), "--parallel", &jobs]));

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
    let src_dir = ws.join("shim/qt6");

    let plugin_dir_str = plugin_dir
        .map(|p| p.to_str().unwrap().to_string())
        .unwrap_or_default();

    println!("    Configuring...");
    let ok = Command::new("cmake")
        .args([
            "-S",
            src_dir.to_str().unwrap(),
            "-B",
            build_dir.to_str().unwrap(),
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
    run(Command::new("cmake").args(["--build", build_dir.to_str().unwrap(), "--parallel", &jobs]));

    println!("    Installing{}...", if use_sudo { " (sudo)" } else { "" });
    let install_args = ["--install", build_dir.to_str().unwrap()];
    if use_sudo {
        run_as_root(Command::new("cmake").args(install_args));
    } else {
        run(Command::new("cmake").args(install_args));
    }
}

// ── Qt6 settings GUI ────────────────────────────────────────────────────────────

/// Builds and installs the Qt6 settings GUI binary.  Returns `true` only if the
/// binary was actually installed (so the caller can skip the launcher when the
/// build was skipped or failed).
fn install_gui(ws: &Path, mode: &Mode) -> bool {
    println!("==> Building Qt6 settings GUI...");

    if !cmd_exists("cmake") {
        println!("    [SKIP] cmake not found.");
        return false;
    }

    let prefix = mode.daemon_prefix();
    let use_sudo = matches!(mode, Mode::System);
    // Allow fetching Corrosion for developer installs, but not in packaging
    // (`--prefix`) mode, which is expected to be reproducible / offline.
    let fetch_corrosion = !mode.is_packaging();
    install_gui_cmake(ws, &prefix, use_sudo, fetch_corrosion)
}

/// Returns `true` if configure/build/install all succeeded.
fn install_gui_cmake(ws: &Path, prefix: &Path, use_sudo: bool, fetch_corrosion: bool) -> bool {
    let build_dir = ws.join("target/xtask-build/qt6-settings");
    let src_dir = ws.join("shim/qt6-settings");

    println!("    Configuring...");
    // Pass paths via OsStr (`.arg`) so non-UTF-8 prefixes are preserved with no
    // lossy Display conversion and no panic.
    let mut prefix_arg = OsString::from("-DCMAKE_INSTALL_PREFIX=");
    prefix_arg.push(prefix);
    let mut configure_cmd = Command::new("cmake");
    configure_cmd
        .arg("-S")
        .arg(&src_dir)
        .arg("-B")
        .arg(&build_dir)
        .arg("-DCMAKE_BUILD_TYPE=Release")
        .arg(&prefix_arg);
    if fetch_corrosion {
        // Developer convenience: fetch Corrosion if it isn't installed.  In
        // packaging mode the CMake default (OFF) is kept so the build fails
        // clearly instead of doing an implicit network fetch.
        configure_cmd.arg("-DY2SKK_FETCH_CORROSION=ON");
    }
    let configured = run_ok(&mut configure_cmd);
    if !configured {
        println!("    [SKIP] cmake configure failed (Qt6 Widgets or Corrosion missing?).");
        return false;
    }

    println!("    Building...");
    let jobs = parallelism();
    let mut build_cmd = Command::new("cmake");
    build_cmd
        .arg("--build")
        .arg(&build_dir)
        .arg("--parallel")
        .arg(&jobs);
    if !run_ok(&mut build_cmd) {
        println!("    [SKIP] cmake build failed.");
        return false;
    }

    println!(
        "    Installing{} -> {}/bin/y2skk-settings-qt6...",
        if use_sudo { " (sudo)" } else { "" },
        prefix.display()
    );
    let mut install_cmd = Command::new("cmake");
    install_cmd.arg("--install").arg(&build_dir);
    let installed = if use_sudo {
        run_as_root_ok(&mut install_cmd)
    } else {
        run_ok(&mut install_cmd)
    };
    if !installed {
        println!("    [SKIP] cmake install failed.");
        return false;
    }
    true
}

/// Install the settings-GUI launcher (`.desktop`) to the user applications dir.
fn install_gui_desktop(ws: &Path, prefix: &Path) {
    let bin_path = prefix.join("bin/y2skk-settings-qt6");

    let app_src = ws.join("dist/applications/y2skk-settings-qt6.desktop");
    let app_content = fs::read_to_string(&app_src)
        .unwrap_or_else(|e| die(&format!("read {}: {e}", app_src.display())));
    // Desktop Entry spec: quote the Exec value so paths with spaces work, and
    // backslash-escape `\` and `"` inside the quotes.
    // Escape `\` and `"` for quoting, and `%` as `%%` (Exec field codes).
    let escaped = bin_path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    let exec_value = format!("\"{escaped}\"");
    let app_content = app_content.replace("%Y2SKK_SETTINGS_QT6_BIN%", &exec_value);
    let app_dest = home_dir().join(".local/share/applications/y2skk-settings-qt6.desktop");
    write_user_file(&app_dest, &app_content);
    println!("  Installed settings launcher: {}", app_dest.display());
}

// ── systemd services ───────────────────────────────────────────────────────────

fn install_daemon_service(ws: &Path, prefix: &Path) {
    let src = ws.join("dist/systemd/y2skk-daemon.service");
    let bin_path = prefix.join("bin/y2skk-daemon");

    let content =
        fs::read_to_string(&src).unwrap_or_else(|e| die(&format!("read {}: {e}", src.display())));
    let content = content.replace("%h/.cargo/bin/y2skk-daemon", bin_path.to_str().unwrap());

    install_systemd_unit("y2skk-daemon.service", &content);
}

fn install_xim_service(ws: &Path, prefix: &Path) {
    let src = ws.join("dist/systemd/y2skk-xim.service");
    let bin_path = prefix.join("bin/y2skk-xim");

    let content =
        fs::read_to_string(&src).unwrap_or_else(|e| die(&format!("read {}: {e}", src.display())));
    let content = content.replace("%h/.cargo/bin/y2skk-xim", bin_path.to_str().unwrap());

    install_systemd_unit("y2skk-xim.service", &content);
}

fn install_systemd_unit(filename: &str, content: &str) {
    let dest_dir = home_dir().join(".config/systemd/user");
    let dest = dest_dir.join(filename);

    println!("==> Installing systemd user service -> {}", dest.display());
    fs::create_dir_all(&dest_dir)
        .unwrap_or_else(|e| die(&format!("create {}: {e}", dest_dir.display())));
    fs::write(&dest, content).unwrap_or_else(|e| die(&format!("write {filename}: {e}")));

    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
}

// ── D-Bus activation ──────────────────────────────────────────────────────────

fn install_dbus_activation(ws: &Path, prefix: &Path) {
    let src = ws.join("dist/dbus/org.y2skk.Daemon.service");
    let bin_path = prefix.join("bin/y2skk-daemon");

    let content =
        fs::read_to_string(&src).unwrap_or_else(|e| die(&format!("read {}: {e}", src.display())));
    // Replace the placeholder binary path with the actual installed path.
    let content = content.replace("%h/.local/bin/y2skk-daemon", bin_path.to_str().unwrap());

    let dest_dir = home_dir().join(".local/share/dbus-1/services");
    let dest = dest_dir.join("org.y2skk.Daemon.service");

    println!("==> Installing D-Bus activation file -> {}", dest.display());
    fs::create_dir_all(&dest_dir)
        .unwrap_or_else(|e| die(&format!("create {}: {e}", dest_dir.display())));
    fs::write(&dest, content).unwrap_or_else(|e| die(&format!("write D-Bus activation: {e}")));
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
    let bin_ver =
        pkg_config_var("gtk+-3.0", "gtk_binary_version").unwrap_or_else(|| "3.0.0".to_string());
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

    // GTK4 module. After removing the .so we re-run gio-querymodules so the
    // GIO module cache no longer references the deleted module. The system
    // path lives under a root-owned directory, so both the file removal and
    // the cache regeneration must go through sudo there.
    let gtk4_path = match &opts.mode {
        Mode::UserLocal => home_dir().join(".local/lib/gtk-4.0/immodules/libim-y2skk.so"),
        Mode::System => gtk4_system_module_path(),
        Mode::Packaging(prefix) => prefix.join("lib/gtk-4.0").join("immodules/libim-y2skk.so"),
    };
    if gtk4_path.exists() {
        println!("  Removing GTK4 module: {}", gtk4_path.display());
        if matches!(opts.mode, Mode::System) {
            run_as_root(Command::new("rm").arg("-f").arg(&gtk4_path));
        } else {
            fs::remove_file(&gtk4_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        }
        removed = true;

        // Regenerate the GIO cache so the deleted entry vanishes. Skipped
        // for packaging mode (the module was never registered with the
        // user's GIO at install time anyway).
        if !matches!(opts.mode, Mode::Packaging(_)) {
            if let Some(parent) = gtk4_path.parent() {
                if matches!(opts.mode, Mode::System) {
                    println!(
                        "    Updating GIO module cache (sudo) -> {}/giomodule.cache",
                        parent.display()
                    );
                    run_as_root(Command::new("gio-querymodules").arg(parent));
                } else {
                    update_gio_module_cache(parent);
                }
            }
        }
    }

    // Qt6 plugin.
    let qt6_path = match &opts.mode {
        Mode::UserLocal => {
            home_dir().join(".local/lib/qt6/plugins/platforminputcontexts/libqy2skk-qt6-plugin.so")
        }
        Mode::System => qt6_system_plugin_path(),
        Mode::Packaging(prefix) => {
            prefix.join("lib/qt6/plugins/platforminputcontexts/libqy2skk-qt6-plugin.so")
        }
    };
    if qt6_path.exists() {
        println!("  Removing Qt6 plugin: {}", qt6_path.display());
        fs::remove_file(&qt6_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        removed = true;
    }

    // XIM server binary.
    let xim_path = opts.mode.daemon_prefix().join("bin/y2skk-xim");
    if xim_path.exists() {
        println!("  Removing XIM server: {}", xim_path.display());
        fs::remove_file(&xim_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        removed = true;
    }

    // Wayland adapter binary.
    let wayland_path = opts.mode.daemon_prefix().join("bin/y2skk-wayland");
    if wayland_path.exists() {
        println!("  Removing Wayland adapter: {}", wayland_path.display());
        fs::remove_file(&wayland_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        removed = true;
    }

    // Settings GUI binary and its launcher (launcher is always user-local).
    let gui_path = opts.mode.daemon_prefix().join("bin/y2skk-settings-qt6");
    if gui_path.exists() {
        println!("  Removing settings GUI: {}", gui_path.display());
        // System mode installs to /usr/local/bin (root-owned); remove via sudo.
        if matches!(opts.mode, Mode::System) {
            run_as_root(Command::new("rm").arg("-f").arg(&gui_path));
        } else {
            fs::remove_file(&gui_path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        }
        removed = true;
    }
    // The launcher is only installed (user-local) in non-packaging mode, so a
    // packaging uninstall must not touch the invoking user's environment.
    if !opts.mode.is_packaging() {
        let gui_desktop = home_dir().join(".local/share/applications/y2skk-settings-qt6.desktop");
        if gui_desktop.exists() {
            println!("  Removing settings launcher: {}", gui_desktop.display());
            fs::remove_file(&gui_desktop).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
            removed = true;
        }
    }

    // Wayland virtual-keyboard desktop file (always user-local).  Older
    // installs may also have left an autostart entry around — clean it up too.
    let wayland_desktop_files = [
        home_dir().join(".local/share/applications/y2skk-wayland.desktop"),
        home_dir().join(".config/autostart/y2skk-wayland-activate.desktop"),
    ];
    for path in &wayland_desktop_files {
        if path.exists() {
            println!("  Removing desktop file: {}", path.display());
            fs::remove_file(path).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
            removed = true;
        }
    }

    // Kana tables directory.
    let tables_dir = opts.mode.daemon_prefix().join("share/y2skk/tables");
    if tables_dir.exists() {
        println!("  Removing kana tables: {}", tables_dir.display());
        if matches!(opts.mode, Mode::System) {
            let _ = Command::new("sudo")
                .args(["rm", "-rf"])
                .arg(&tables_dir)
                .status();
        } else {
            fs::remove_dir_all(&tables_dir).unwrap_or_else(|e| eprintln!("  Warning: {e}"));
        }
        removed = true;
    }

    // Systemd services (always user-local).  y2skk-wayland.service is legacy
    // — earlier versions of xtask installed one, but the Wayland adapter is
    // now launched by KWin's virtual-keyboard machinery instead.  Clean up
    // the file if it's left over from a previous install.
    let mut need_reload = false;
    for name in &[
        "y2skk-daemon.service",
        "y2skk-xim.service",
        "y2skk-wayland.service",
    ] {
        let service = home_dir().join(".config/systemd/user").join(name);
        if service.exists() {
            println!("  Removing systemd service: {}", service.display());
            fs::remove_file(&service).ok();
            need_reload = true;
            removed = true;
        }
    }
    if need_reload {
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
    }

    // D-Bus activation file (always user-local).
    let dbus_act = home_dir().join(".local/share/dbus-1/services/org.y2skk.Daemon.service");
    if dbus_act.exists() {
        println!("  Removing D-Bus activation file: {}", dbus_act.display());
        fs::remove_file(&dbus_act).ok();
        removed = true;
    }

    if removed {
        println!("Uninstall complete.");
    } else {
        println!("Nothing to uninstall.");
    }
}

fn gtk3_system_module_path() -> PathBuf {
    let libdir = pkg_config_var("gtk+-3.0", "libdir").unwrap_or_else(|| "/usr/lib".to_string());
    let bin_ver =
        pkg_config_var("gtk+-3.0", "gtk_binary_version").unwrap_or_else(|| "3.0.0".to_string());
    PathBuf::from(libdir)
        .join("gtk-3.0")
        .join(&bin_ver)
        .join("immodules/im-y2skk.so")
}

fn gtk4_system_module_path() -> PathBuf {
    let libdir = pkg_config_var("gtk4", "libdir").unwrap_or_else(|| "/usr/lib".to_string());
    PathBuf::from(libdir)
        .join("gtk-4.0")
        .join("immodules/libim-y2skk.so")
}

fn qt6_system_plugin_path() -> PathBuf {
    let base =
        qmake_query("QT_INSTALL_PLUGINS").unwrap_or_else(|| "/usr/lib/qt6/plugins".to_string());
    PathBuf::from(base).join("platforminputcontexts/libqy2skk-qt6-plugin.so")
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn parallelism() -> String {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .to_string()
}

/// Copy `src` to `dest` atomically, creating parent directories as needed.
/// Writes to a sibling temp file first, then renames over `dest` so a running
/// binary keeps using its open inode and there is no "Text file busy" failure.
/// Works the same when `dest` does not yet exist.
fn install_file(src: &Path, dest: &Path, use_sudo: bool) {
    println!("    {} -> {}", src.display(), dest.display());
    let dest_dir = dest.parent().unwrap();
    let tmp_name = format!(
        ".{}.xtask-tmp.{}",
        dest.file_name().unwrap().to_string_lossy(),
        std::process::id(),
    );
    let tmp = dest_dir.join(&tmp_name);

    if use_sudo {
        // `install -D` creates parent dirs and sets mode in one shot.
        run_as_root(Command::new("install").args([
            "-D",
            "-m",
            "755",
            src.to_str().unwrap(),
            tmp.to_str().unwrap(),
        ]));
        // Atomic replace.  `mv -f` succeeds whether or not `dest` exists.
        run_as_root(Command::new("mv").args(["-f", tmp.to_str().unwrap(), dest.to_str().unwrap()]));
    } else {
        fs::create_dir_all(dest_dir)
            .unwrap_or_else(|e| die(&format!("create {}: {e}", dest_dir.display())));
        fs::copy(src, &tmp).unwrap_or_else(|e| die(&format!("copy to {}: {e}", tmp.display())));
        set_executable(&tmp);
        fs::rename(&tmp, dest)
            .unwrap_or_else(|e| die(&format!("rename to {}: {e}", dest.display())));
    }
}

fn run(cmd: &mut Command) {
    let prog = cmd.get_program().to_string_lossy().to_string();
    let status = cmd
        .status()
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

/// Like `run`, but returns whether the command succeeded instead of aborting.
/// Used where a failed step should be skipped (returning `false`) rather than
/// terminating the whole install.
fn run_ok(cmd: &mut Command) -> bool {
    match cmd.status() {
        Ok(status) => status.success(),
        Err(e) => {
            let prog = cmd.get_program().to_string_lossy().to_string();
            eprintln!("    failed to run {prog}: {e}");
            false
        }
    }
}

/// Like `run_as_root`, but returns success instead of aborting on failure.
fn run_as_root_ok(cmd: &mut Command) -> bool {
    let prog: OsString = cmd.get_program().to_os_string();
    let args: Vec<OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
    let mut sudo_cmd = Command::new("sudo");
    sudo_cmd.arg(prog);
    sudo_cmd.args(args);
    run_ok(&mut sudo_cmd)
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
