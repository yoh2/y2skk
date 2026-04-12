# Installing y2skk

This guide covers building and installing y2skk on Linux.

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
| Qt6 development headers (including private headers) | Qt6 IM plugin |

The GTK3 and Qt6 packages are only required if you want those adapters.
The daemon itself and the XIM server have no additional system dependencies beyond Rust.

### SKK dictionary

y2skk requires at least one SKK dictionary.
Download from [skk-dev/dict](https://github.com/skk-dev/dict), or install via your
distribution's package manager.

---

## 2. Build and install

### Standard install

```sh
cargo xtask install
```

The daemon and systemd service install to `~/.local/`; the GTK3 and Qt6 adapters
install to the system directories reported by `pkg-config` / `qmake`.
**Sudo is required** for the adapter steps — you will be prompted automatically.

| Component | Installed path |
|-----------|---------------|
| `y2skk-daemon` | `~/.local/bin/y2skk-daemon` |
| GTK3 IM module | `<pkg-config libdir>/gtk-3.0/<binver>/immodules/im-y2skk.so` |
| Qt6 IM plugin | `<qmake QT_INSTALL_PLUGINS>/platforminputcontexts/libqy2skk-qt6-plugin.so` |
| systemd service | `~/.config/systemd/user/y2skk-daemon.service` |

Also refreshes the system GTK3 IM module cache via `gtk-query-immodules-3.0 --update-cache`.

### Install specific components only

```sh
cargo xtask install --daemon   # daemon + systemd service only
cargo xtask install --gtk3     # GTK3 IM module only (sudo for system install)
cargo xtask install --qt6      # Qt6 plugin only (sudo for system install)
```

### Packaging

Use `--prefix` to install all components into a staging directory without sudo.
The IM module cache update is skipped automatically in this mode.

```sh
cargo xtask install --prefix /path/to/staging/usr
```

This installs:

| Component | Installed path |
|-----------|---------------|
| `y2skk-daemon` | `<prefix>/bin/y2skk-daemon` |
| GTK3 IM module | `<prefix>/lib/gtk-3.0/<binver>/immodules/im-y2skk.so` |
| Qt6 IM plugin | `<prefix>/lib/qt6/plugins/platforminputcontexts/libqy2skk-qt6-plugin.so` |

The systemd service file is **not** installed in packaging mode.

---

## 3. Environment variables

Set the following in your shell profile or session startup script
(`~/.bash_profile`, `~/.profile`, or the equivalent for your desktop environment):

```sh
# XIM clients (xterm, Chromium, legacy X11 apps)
export XMODIFIERS=@im=y2skk

# GTK3 applications
export GTK_IM_MODULE=y2skk

# Qt6 applications
export QT_IM_MODULE=y2skk
```

For **KDE Plasma**, place these in `~/.config/plasma-workspace/env/y2skk.sh`
(executed automatically at login). Log out and back in to apply.

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

## 5. Start the daemon

```sh
systemctl --user enable --now y2skk-daemon
```

Check the status:

```sh
systemctl --user status y2skk-daemon
```

View live logs:

```sh
journalctl --user -u y2skk-daemon -f
```

For more verbose output:

```sh
RUST_LOG=debug systemctl --user restart y2skk-daemon
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
cargo xtask uninstall
```

This removes all files installed by `cargo xtask install`.

For a custom prefix:

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

1. Verify the environment variable is set: `echo $GTK_IM_MODULE`
2. Verify the system IM module cache contains y2skk: `grep y2skk /etc/gtk-3.0/gtk.immodules`
3. Re-run the cache update as root if needed: `sudo gtk-query-immodules-3.0 --update-cache`

### Qt6 apps do not use y2skk

1. Verify `QT_IM_MODULE=y2skk` is set.
2. Verify the plugin file exists in the system Qt6 plugin directory:
   ```sh
   find "$(qmake6 -query QT_INSTALL_PLUGINS)" -name 'libqy2skk*.so' 2>/dev/null
   ```

### Reload configuration

Send a reload request over D-Bus (no daemon restart needed):

```sh
busctl call --user org.y2skk.Daemon /org/y2skk/Daemon org.y2skk.Daemon ReloadConfig
```
