# Installing y2skk

This guide covers building and installing y2skk on Linux.

:warning: This installation method is likely to fail.

---

## 1. Requirements

### Rust toolchain

Install Rust via [rustup](https://rustup.rs/) or your distribution's package manager.

### System packages

You need the following tools and libraries, available from your distribution's package manager:

| Package | Required for |
|---------|-------------|
| `cmake` ≥ 3.21 | Qt6 plugin build |
| `pkg-config` | Build system |
| GTK3 development headers | GTK3 IM module |
| GTK4 development headers | GTK4 IM module |
| Qt6 development headers (including private headers) | Qt6 IM plugin |
| `gio-querymodules` (usually shipped with `glib2`) | Updating the GIO module cache after installing the GTK4 module |

The GTK3, GTK4, and Qt6 packages are only required if you want those adapters.
The daemon itself and the XIM server have no additional system dependencies beyond Rust.

### SKK dictionary

y2skk requires at least one SKK dictionary.
Download from [skk-dev/dict](https://github.com/skk-dev/dict), or install via your
distribution's package manager.

---

## 2. Build and install

Three install modes are available.

### User-local install (default, no sudo required)

```sh
cargo xtask install
```

The default install builds the daemon, the XIM server, the GTK3 / GTK4 / Qt6
adapters, and the experimental Wayland adapter, under `~/.local/`.
The GTK3, GTK4, and Qt6 adapters require additional environment variables
(printed at the end of install).

| Component | Installed path |
|-----------|---------------|
| `y2skk-daemon` | `~/.local/bin/y2skk-daemon` |
| `y2skk-xim` | `~/.local/bin/y2skk-xim` |
| `y2skk-wayland` | `~/.local/bin/y2skk-wayland` |
| Kana tables | `~/.local/share/y2skk/tables/` |
| GTK3 IM module | `~/.local/lib/gtk-3.0/<binver>/immodules/im-y2skk.so` |
| GTK4 IM module | `~/.local/lib/gtk-4.0/immodules/libim-y2skk.so` |
| Qt6 IM plugin | `~/.local/lib/qt6/plugins/platforminputcontexts/libqy2skk-qt6-plugin.so` |
| systemd services | `~/.config/systemd/user/y2skk-daemon.service`, `y2skk-xim.service` |
| KDE Virtual Keyboard entry | `~/.local/share/applications/y2skk-wayland.desktop` |

Also updates the GTK3 user module cache (`~/.config/gtk-3.0/gtk.immodules`)
and runs `gio-querymodules` against the GTK4 immodule directory
(`~/.local/lib/gtk-4.0/immodules/giomodule.cache`).

### System-wide install (sudo required for adapters)

```sh
cargo xtask install --system
```

Daemon installs to `/usr/local/bin/`; adapters install to the system directories
reported by `pkg-config` / `qmake`. Sudo is invoked automatically for those steps.
No extra environment variables are needed after this.

| Component | Installed path |
|-----------|---------------|
| `y2skk-daemon` | `/usr/local/bin/y2skk-daemon` |
| `y2skk-xim` | `/usr/local/bin/y2skk-xim` |
| `y2skk-wayland` | `/usr/local/bin/y2skk-wayland` |
| Kana tables | `/usr/local/share/y2skk/tables/` |
| GTK3 IM module | `<pkg-config libdir>/gtk-3.0/<binver>/immodules/im-y2skk.so` |
| GTK4 IM module | `<pkg-config libdir>/gtk-4.0/immodules/libim-y2skk.so` |
| Qt6 IM plugin | `<qmake QT_INSTALL_PLUGINS>/platforminputcontexts/libqy2skk-qt6-plugin.so` |
| systemd services | `~/.config/systemd/user/y2skk-daemon.service`, `y2skk-xim.service` |
| KDE Virtual Keyboard entry | `~/.local/share/applications/y2skk-wayland.desktop` |

### Install specific components only

```sh
cargo xtask install --daemon          # daemon + systemd service only
cargo xtask install --xim             # XIM server + systemd service only (user-local)
cargo xtask install --system --xim    # XIM server to system path (sudo)
cargo xtask install --gtk3            # GTK3 IM module only (user-local)
cargo xtask install --system --gtk3   # GTK3 to system path (sudo)
cargo xtask install --gtk4            # GTK4 IM module only (user-local)
cargo xtask install --system --gtk4   # GTK4 to system path (sudo)
cargo xtask install --qt6             # Qt6 plugin only (user-local)
cargo xtask install --wayland         # Wayland adapter + KDE Virtual Keyboard entry
```

> **Note:** the Wayland adapter is **experimental** and KWin-only — see the
> Wayland section in [README.md](README.md#wayland-support-experimental-but-improving)
> for the current status. It is included in the default install; if your
> session is on KDE Plasma Wayland, activate the adapter under
> *System Settings → Keyboard → Virtual Keyboard → y2skk* after install.

### Packaging

Use `--prefix` to install all components into a staging directory.
No sudo is used, and the IM module cache update is skipped.

```sh
cargo xtask install --prefix /path/to/staging/usr
```

| Component | Installed path |
|-----------|---------------|
| `y2skk-daemon` | `<prefix>/bin/y2skk-daemon` |
| `y2skk-xim` | `<prefix>/bin/y2skk-xim` |
| `y2skk-wayland` | `<prefix>/bin/y2skk-wayland` |
| Kana tables | `<prefix>/share/y2skk/tables/` |
| GTK3 IM module | `<prefix>/lib/gtk-3.0/<binver>/immodules/im-y2skk.so` |
| GTK4 IM module | `<prefix>/lib/gtk-4.0/immodules/libim-y2skk.so` |
| Qt6 IM plugin | `<prefix>/lib/qt6/plugins/platforminputcontexts/libqy2skk-qt6-plugin.so` |

The systemd service file and the KDE Virtual Keyboard `.desktop` entry are **not**
installed in packaging mode.

---

## 3. Environment variables

Set the following in your shell profile or session startup script
(`~/.bash_profile`, `~/.profile`, or the equivalent for your desktop environment).

For **KDE Plasma**, place these in `~/.config/plasma-workspace/env/y2skk.sh`
(executed automatically at login). Log out and back in to apply.

### System-wide install

```sh
export XMODIFIERS=@im=y2skk    # XIM clients (xterm, Chromium, legacy X11 apps)
export GTK_IM_MODULE=y2skk     # GTK3 / GTK4 applications
export QT_IM_MODULE=y2skk      # Qt6 applications
```

### User-local install (default)

The same variables as above, plus these for the user-local adapter paths:

```sh
export XMODIFIERS=@im=y2skk
export GTK_IM_MODULE=y2skk
export QT_IM_MODULE=y2skk
export GTK_IM_MODULE_FILE="$HOME/.config/gtk-3.0/gtk.immodules"
export GIO_EXTRA_MODULES="$HOME/.local/lib/gtk-4.0/immodules:$GIO_EXTRA_MODULES"
export QT_PLUGIN_PATH="$HOME/.local/lib/qt6/plugins:$QT_PLUGIN_PATH"
```

`GIO_EXTRA_MODULES` is needed because GIO does not scan
`~/.local/lib/gtk-4.0/immodules/` by default; without it, `GTK_IM_MODULE=y2skk`
would still produce a "no IM module matching y2skk found" warning in GTK4 apps.

---

## 4. Configure a dictionary

Copy the example configuration and edit the `[[dict.sources]]` section:

```sh
mkdir -p ~/.config/y2skk
cp dist/config.toml.example ~/.config/y2skk/config.toml
$EDITOR ~/.config/y2skk/config.toml
```

Uncomment and set the dictionary path, for example:

```toml
[[dict.sources]]
path = "/usr/share/skk/SKK-JISYO.L"
encoding = "euc-jp"
priority = 0
```

See [`dist/config.toml.example`](dist/config.toml.example) for all available options.

---

## 5. Start the services

```sh
systemctl --user enable --now y2skk-daemon
systemctl --user enable --now y2skk-xim
```

`y2skk-daemon` handles all input processing and dictionary lookups.
`y2skk-xim` is the XIM server for legacy X11 clients (xterm, Chromium, etc.); it connects to the daemon via D-Bus.

The experimental Wayland adapter `y2skk-wayland` is **not** a systemd service.
KWin starts it on demand via the Virtual Keyboard mechanism; enable it under
*System Settings → Keyboard → Virtual Keyboard → y2skk*. (KDE remembers the
selection across sessions.)

Check the status:

```sh
systemctl --user status y2skk-daemon y2skk-xim
```

View live logs:

```sh
journalctl --user -u y2skk-daemon -f
journalctl --user -u y2skk-xim -f
```

For more verbose output:

```sh
RUST_LOG=debug systemctl --user restart y2skk-daemon y2skk-xim
```

### Daemon command-line options

| Option | Description |
|--------|-------------|
| `--config <PATH>` / `-c <PATH>` | Load config from `<PATH>` instead of the default XDG location |
| `--check-config` | Validate the config file and exit without starting the daemon (exit code 0 = OK, 1 = error) |

These options can be combined:

```sh
y2skk-daemon --check-config
y2skk-daemon --check-config --config /tmp/my.toml
```

If you use a non-default config path with systemd, override `ExecStart` in a drop-in file:

```sh
systemctl --user edit y2skk-daemon
# Add:
# [Service]
# ExecStart=
# ExecStart=%h/.local/bin/y2skk-daemon --config /path/to/config.toml
```

---

## 6. Quick test

Open an XIM client and try typing:

```sh
XMODIFIERS=@im=y2skk xterm
```

Type `a` to get `あ`, then `Space` to start conversion.

---

## 7. Uninstall

```sh
cargo xtask uninstall           # user-local (default)
cargo xtask uninstall --system  # system-wide install
```

For a packaging prefix:

```sh
cargo xtask uninstall --prefix /path/to/staging/usr
```

---

## Troubleshooting

### Daemon does not start

Check the log:

```sh
journalctl --user -u y2skk-daemon -n 50
```

Common causes:
- No dictionary configured — the daemon starts but conversion will find no candidates.
- D-Bus session bus not running.

### GTK3 apps do not use y2skk

1. Verify `GTK_IM_MODULE=y2skk` is set: `echo $GTK_IM_MODULE`
2. **User-local install**: verify `GTK_IM_MODULE_FILE` points to the user cache and the cache contains y2skk:
   ```sh
   echo $GTK_IM_MODULE_FILE
   grep y2skk "$GTK_IM_MODULE_FILE"
   ```
   Re-run the cache update if needed:
   ```sh
   cargo xtask install --gtk3
   ```
3. **System install**: verify the system cache: `grep y2skk /etc/gtk-3.0/gtk.immodules`
   Re-run as root: `sudo gtk-query-immodules-3.0 --update-cache`

### GTK4 apps do not use y2skk

1. Verify `GTK_IM_MODULE=y2skk` is set: `echo $GTK_IM_MODULE`
2. **User-local install**: verify `GIO_EXTRA_MODULES` includes the user
   immodule directory:
   ```sh
   echo $GIO_EXTRA_MODULES
   ls "$HOME/.local/lib/gtk-4.0/immodules/libim-y2skk.so"
   ```
   If the cache file is missing, regenerate it:
   ```sh
   gio-querymodules "$HOME/.local/lib/gtk-4.0/immodules"
   ```
   (or just re-run `cargo xtask install --gtk4`).
3. **System install**: verify the module is in the system path and the cache
   knows about it:
   ```sh
   ls "$(pkg-config --variable=libdir gtk4)/gtk-4.0/immodules/libim-y2skk.so"
   grep y2skk "$(pkg-config --variable=libdir gtk4)/gtk-4.0/immodules/giomodule.cache"
   ```
   Re-run as root to refresh: `sudo gio-querymodules "$(pkg-config --variable=libdir gtk4)/gtk-4.0/immodules"`
4. Confirm the module loads at runtime:
   ```sh
   GTK_DEBUG=modules GTK_IM_MODULE=y2skk gnome-text-editor 2>&1 | grep -i y2skk
   ```
   You should see the module path in the output. Note: GTK4 also requires the
   y2skk daemon to be running (`systemctl --user status y2skk-daemon`).

### Qt6 apps do not use y2skk

1. Verify `QT_IM_MODULE=y2skk` is set.
2. **User-local install**: verify `QT_PLUGIN_PATH` includes `~/.local/lib/qt6/plugins`.
3. **System install**: verify the plugin file exists:
   ```sh
   find "$(qmake6 -query QT_INSTALL_PLUGINS)" -name 'libqy2skk*.so' 2>/dev/null
   ```

### Daemon fails to start: kana table not found

If the log shows a "kana table not found" error:

```sh
journalctl --user -u y2skk-daemon -n 20
```

The daemon searches for `<kana_layout>.txt` (default `romaji.txt`) in:

1. `~/.config/y2skk/tables/`
2. `~/.local/share/y2skk/tables/`
3. Directories in `$XDG_DATA_DIRS` (e.g. `/usr/local/share/y2skk/tables/`, `/usr/share/y2skk/tables/`)

Reinstall the daemon to restore the tables:

```sh
cargo xtask install --daemon          # user-local
cargo xtask install --system --daemon # system-wide (sudo)
```

To use a custom table file, set `kana_table` in `config.toml`:

```toml
[input]
kana_table = "~/.config/y2skk/tables/my-layout.txt"
```

### XIM clients do not use y2skk

1. Verify `XMODIFIERS=@im=y2skk` is set: `echo $XMODIFIERS`
2. Verify `y2skk-xim` is running:
   ```sh
   systemctl --user status y2skk-xim
   ```
   If it is not running, enable and start it:
   ```sh
   systemctl --user enable --now y2skk-xim
   ```
3. Check the XIM server log for errors:
   ```sh
   journalctl --user -u y2skk-xim -n 50
   ```

### Wayland adapter does not work (KDE Plasma, experimental)

The Wayland adapter is **experimental** and KWin-only; see
[README.md](README.md#wayland-support-experimental-but-improving) for the
current status. Some basic checks:

1. Confirm you are running on KWin (KDE Plasma 5/6) under a Wayland session.
   Other compositors are not supported.
2. Verify the binary is installed: `ls ~/.local/bin/y2skk-wayland` (or
   `/usr/local/bin/y2skk-wayland` for a system install).
3. Verify the Virtual Keyboard entry is present:
   ```sh
   ls ~/.local/share/applications/y2skk-wayland.desktop
   ```
   If missing, re-run `cargo xtask install --wayland`.
4. In *System Settings → Keyboard → Virtual Keyboard*, make sure **y2skk** is
   selected, then log out and back in.
5. Check the daemon log — `y2skk-wayland` connects through it:
   ```sh
   journalctl --user -u y2skk-daemon -f
   ```
6. `y2skk-wayland` itself is launched by KWin; its stderr/stdout is captured by the
   compositor, so consult `journalctl --user -b` or
   `journalctl --user-unit plasma-kwin_wayland.service -f` if available.

### Reload configuration

Send a reload request over D-Bus (no daemon restart needed):

```sh
busctl call --user org.y2skk.Daemon /org/y2skk/Daemon org.y2skk.Daemon ReloadConfig
```
