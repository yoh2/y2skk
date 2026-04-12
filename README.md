# y2skk

> **⚠ Work in Progress**
>
> y2skk is under active development. Breaking changes may occur between versions,
> and some features listed below may be incomplete or missing.
> Use at your own risk in production environments.

A [SKK](https://skk-dev.github.io/skk/) Japanese input method for Linux, written in Rust.

y2skk runs as a daemon (`y2skk-daemon`) and exposes its functionality over D-Bus.
Platform-specific adapters (GTK3, Qt6, XIM) plug in to the daemon so that a single
dictionary and session state are shared across all applications.

**[日本語版 README → README_ja.md](README_ja.md)**

---

## Supported Environments

| Component | Status |
|-----------|--------|
| GTK3 applications | ✅ Working |
| Qt6 applications | ✅ Working |
| XIM clients (xterm, etc.) | ✅ Working (integrated into daemon) |
| KDE Plasma (X11) | ✅ Primary target |
| Wayland / GTK4 | 🚧 Not yet implemented |

---

## Features

- **SKK protocol** — hiragana / katakana / half-width katakana / wide-ASCII / ASCII modes
- **Kana input layouts** — Romaji, AZIK (US/JP), DvorakJP (US/JP)
- **Dictionary support** — UTF-8 and EUC-JP / EUC-JISX0213 dictionaries; multiple system dictionaries with configurable priority
- **User dictionary** — word registration (`▼` mode), automatic save on commit
- **Candidate selection** — inline display (configurable count), then list mode with selection keys
- **Tab completion** — ghost-text completion in `▽` mode (uim-skk style)
- **IME toggle** — Shift+Space toggles between hiragana and ASCII mode (configurable)
- **Mode indicator** — floating popup on mode change (auto-hide, configurable timeout)
- **Code input** — `\XXXX` (JIS) and `\uXXXX` (Unicode) character input
- **Abbrev mode** — ASCII romaji search (`/` key)
- **XIM server** — built into the daemon, no separate binary needed

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

### 1. Build and install

```sh
cargo xtask install
```

This builds all components and installs them under `~/.local/`.
See [INSTALL.md](INSTALL.md) for details, options, and system-wide installation.

### 2. Set environment variables

For **KDE Plasma**, create `~/.config/plasma-workspace/env/y2skk.sh`:

```sh
export XMODIFIERS=@im=y2skk      # XIM clients (xterm, chromium, …)
export GTK_IM_MODULE=y2skk       # GTK3 applications
export QT_IM_MODULE=y2skk        # Qt6 applications
```

Log out and back in (or run `source` on the file) to apply.

### 3. Configure a dictionary

Copy the example config and edit the dictionary path:

```sh
mkdir -p ~/.config/y2skk
cp dist/config.toml.example ~/.config/y2skk/config.toml
$EDITOR ~/.config/y2skk/config.toml
```

### 4. Start the daemon

```sh
systemctl --user enable --now y2skk-daemon
```

Check the log:

```sh
journalctl --user -u y2skk-daemon -f
```

---

## Configuration

The configuration file lives at `~/.config/y2skk/config.toml`.
See [`dist/config.toml.example`](dist/config.toml.example) for all available options with descriptions.

---

## License

MIT
