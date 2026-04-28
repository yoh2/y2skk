# y2skk

> **⚠ Work in Progress**
>
> y2skk is under active development. Breaking changes may occur between versions,
> and some features listed below may be incomplete or missing.
> Use at your own risk in production environments.

A SKK Japanese input method for Linux, written in Rust.

y2skk runs as a daemon (`y2skk-daemon`) and exposes its functionality over D-Bus.
Platform-specific adapters (GTK3, Qt6, XIM, Wayland) plug in to the daemon so that a
single dictionary and session state are shared across all applications.

**[日本語版 README → README_ja.md](README_ja.md)**

---

## Supported Environments

| Component | Status |
|-----------|--------|
| GTK3 applications | ✅ Working |
| Qt6 applications | ✅ Working |
| XIM clients (xterm, etc.) | ✅ Working (via `y2skk-xim`) |
| KDE Plasma (X11) | ✅ Primary target |
| KDE Plasma (Wayland) | 🧪 **Very early / experimental** — see notes below |
| GTK4 | 🚧 Not yet implemented |

### Wayland support: very early stage

The Wayland adapter (`y2skk-wayland`) is **at a very early experimental stage** and
should not be relied on for daily use. It is built on KDE's `zwp_input_method_v1`
extension, so it currently runs only on KWin (KDE Plasma 5/6) and is not portable to
other Wayland compositors. Known issues at the time of writing include:

- Mishandled events with Chromium-family applications launched with
  `--ozone-platform=wayland` (e.g. backspace works only every other press in some
  text fields).
- No support for `zwp_input_method_v2` / `text-input-v3` yet, so non-KDE compositors
  (GNOME, Sway, etc.) are unsupported.
- Documentation, packaging, and stability are all incomplete.

Use the X11 path (XIM + GTK3 + Qt6 adapters) for production work; treat the Wayland
adapter as a preview only.

---

## Features

- **SKK protocol** — hiragana / katakana / half-width katakana / wide-ASCII / ASCII modes
- **Kana input layouts** — Romaji, AZIK (US/JP), DvorakJP (US/JP)
- **Dictionary support** — UTF-8 and EUC-JP / EUC-JISX0213 dictionaries; multiple system dictionaries with configurable priority
- **User dictionary** — word registration (`▼` mode), automatic save on commit
- **Number conversion** — DDSKK-style numeric templates (`#0`–`#3`, `#5`, `#9`) plus
  y2skk extensions (`#6`, `#7`, `#a`, `#b`, `#c`); synthetic candidates are produced
  even when the dictionary has no entry for the templated reading
- **Candidate selection** — inline display (configurable count), then list mode with selection keys
- **Tab completion** — ghost-text completion in `▽` mode (uim-skk style)
- **IME toggle** — Shift+Space toggles between hiragana and ASCII mode (configurable)
- **Mode indicator** — floating popup on mode change (auto-hide, configurable timeout)
- **Code input** — `\XXXX` (JIS) and `\uXXXX` (Unicode) character input
- **Abbrev mode** — ASCII romaji search (`/` key)
- **vi-compatible Esc** — optional mode; pressing Esc in a normal input phase switches to ASCII mode (configurable)
- **XIM server** — standalone `y2skk-xim` binary that connects to the daemon via D-Bus
- **Wayland adapter (experimental)** — standalone `y2skk-wayland` binary using
  `zwp_input_method_v1` (KDE only); see the warning above
- **Daemon reconnect / fail-open** — adapters keep working when the daemon is
  restarted; D-Bus errors fall back to passthrough rather than blocking the UI
- **Config validation** — `y2skk-daemon --check-config [--config <PATH>]` validates the config file and exits without starting the daemon

---

## Requirements

### Runtime

- A running D-Bus session bus

### Build

| Dependency | Purpose |
|------------|---------|
| Rust + Cargo | Build all components |
| cmake ≥ 3.21 | Build the Qt6 plugin |
| GTK3 dev headers | Build the GTK3 IM module |
| Qt6 + private headers | Build the Qt6 plugin |
| pkg-config | Used by the build system |

Install these via your distribution's package manager.
The GTK3 and Qt6 packages are only required if you want those adapters.

### Dictionary

y2skk requires at least one SKK dictionary.
Download from [skk-dev/dict](https://github.com/skk-dev/dict), or install via your distribution's package manager.

---

## Quick Start

:warning: This installation method is likely to fail.

### 1. Build and install

```sh
cargo xtask install
```

This builds all components (daemon, XIM server, GTK3, Qt6) and installs them under
`~/.local/`. The experimental Wayland adapter is **not** installed by default;
add `--wayland` to opt in:

```sh
cargo xtask install --wayland         # Wayland adapter only
cargo xtask install --daemon --xim --gtk3 --qt6 --wayland   # everything
```

See [INSTALL.md](INSTALL.md) for details, options, and system-wide installation.

### 2. Set environment variables

For **KDE Plasma**, create `~/.config/plasma-workspace/env/y2skk.sh`:

```sh
export XMODIFIERS=@im=y2skk      # XIM clients (xterm, chromium, …)
export GTK_IM_MODULE=y2skk       # GTK3 applications
export QT_IM_MODULE=y2skk        # Qt6 applications
# Additional variables for the default user-local adapter install:
export GTK_IM_MODULE_FILE="$HOME/.config/gtk-3.0/gtk.immodules"
export QT_PLUGIN_PATH="$HOME/.local/lib/qt6/plugins:$QT_PLUGIN_PATH"
```

> With `cargo xtask install --system` the last two lines are not needed.

Log out and back in (or run `source` on the file) to apply.

### 3. Configure a dictionary

Copy the example config and edit the dictionary path:

```sh
mkdir -p ~/.config/y2skk
cp dist/config.toml.example ~/.config/y2skk/config.toml
$EDITOR ~/.config/y2skk/config.toml
```

### 4. Start the services

```sh
systemctl --user enable --now y2skk-daemon
systemctl --user enable --now y2skk-xim
```

Check the log:

```sh
journalctl --user -u y2skk-daemon -f
journalctl --user -u y2skk-xim -f
```

> The Wayland adapter (`y2skk-wayland`) does **not** run as a systemd service; it is
> launched by KWin via the Virtual Keyboard mechanism. After installing it with
> `cargo xtask install --wayland`, enable it under
> *System Settings → Keyboard → Virtual Keyboard → y2skk*.

---

## Configuration

The configuration file lives at `~/.config/y2skk/config.toml`.
See [`dist/config.toml.example`](dist/config.toml.example) for all available options with descriptions.

---

## License

MIT
