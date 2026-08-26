# Third-Party Notices

y2skk itself is licensed under the MIT License (see [`LICENSE`](LICENSE)). It
links against the third-party libraries listed below, which remain under their
own licenses. y2skk links these libraries **dynamically** and does not modify
or redistribute them; they are provided by your system (for example, your Linux
distribution's packages). Because the linking is dynamic, users may replace a
library with a modified version, as required by the LGPL.

## GUI / input-method toolkits

- **Qt 6** — GNU Lesser General Public License v3 (LGPL-3.0)
  Used by the Qt 6 input-method plugin (`adapter-qt6`) and the settings
  application (`y2skk-settings-qt6`). Modules used: QtCore, QtGui, QtWidgets,
  QtConcurrent.
  Homepage / source: <https://www.qt.io/> , <https://code.qt.io/>

- **GTK 3** — GNU Lesser General Public License v2.1 (LGPL-2.1)
  Used by the GTK 3 input-method module (`adapter-gtk3`).
  Homepage / source: <https://www.gtk.org/> , <https://gitlab.gnome.org/GNOME/gtk>

- **GTK 4** — GNU Lesser General Public License v2.1 (LGPL-2.1)
  Used by the GTK 4 input-method module (`adapter-gtk4`).
  Homepage / source: <https://www.gtk.org/> , <https://gitlab.gnome.org/GNOME/gtk>

## Rust dependencies

The Rust crates y2skk depends on (for example zbus, tokio, serde, x11rb,
`wayland-*`, xkbcommon, fontdue) are distributed under permissive licenses,
predominantly MIT and Apache-2.0. They are fetched from crates.io at build
time, and each crate ships its own license text within its source package. For
a full, version-specific listing, run `cargo license` or
`cargo about generate` over the workspace.
