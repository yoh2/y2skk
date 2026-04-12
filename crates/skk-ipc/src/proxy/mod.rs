//! D-Bus proxy helpers for `org.y2skk.Daemon`.
//!
//! This module centralises the constants and call signatures shared by every
//! platform adapter (GTK3, Qt6, XIM, ...).  Adapters pick a variant:
//!
//! - [`blocking::DaemonProxy`] — synchronous, suitable for toolkit callbacks
//!   that cannot await (GTK3 IM module, Qt6 `filterEvent`).
//! - [`nonblocking::DaemonProxy`] — async, used by the tokio-based XIM server.

pub mod blocking;
pub mod nonblocking;

/// Well-known D-Bus bus name of the y2skk daemon.
pub const BUS_NAME: &str = "org.y2skk.Daemon";

/// Object path exposing the daemon interface.
pub const OBJECT_PATH: &str = "/org/y2skk/Daemon";

/// Interface name implemented on [`OBJECT_PATH`].
pub const INTERFACE: &str = "org.y2skk.Daemon";
