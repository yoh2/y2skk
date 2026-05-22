// Config schema, validation and path resolution live in the shared `skk-config`
// crate so the GUI settings tool can reuse them.  Re-exported as `config` so the
// daemon's internal `crate::config::…` references keep working unchanged.
pub use skk_config as config;
pub mod dbus;
pub mod session;
