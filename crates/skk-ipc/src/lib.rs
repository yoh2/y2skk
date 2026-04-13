//! Shared IPC types used by both `skk-daemon` and the platform adapters.
//!
//! All types must be serializable over D-Bus (via `zvariant`).

use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

pub mod convert;
pub mod proxy;

pub type SessionId = u32;

// ── Action kind constants ─────────────────────────────────────────────────────

pub const ACTION_PASSTHROUGH: u8 = 0;
pub const ACTION_COMMIT: u8 = 1;
pub const ACTION_UPDATE_PREEDIT: u8 = 2;
pub const ACTION_CLEAR_PREEDIT: u8 = 3;
pub const ACTION_SHOW_CANDIDATES: u8 = 4;
pub const ACTION_HIDE_CANDIDATES: u8 = 5;
pub const ACTION_UPDATE_STATUS: u8 = 6;
/// Returned by the daemon when the session ID is not recognised (e.g. after a
/// daemon restart).  Adapters should re-create the session and retry the key.
pub const ACTION_SESSION_INVALID: u8 = 7;

// ── IpcAction ─────────────────────────────────────────────────────────────────

/// An action produced by the engine, encoded as a flat D-Bus-compatible struct.
///
/// Fields unused by a particular `kind` are set to zero/empty.
///
/// | kind | text             | cursor        | candidates | focused |
/// |------|------------------|---------------|------------|---------|
/// | 0 Passthrough    | –   | –             | –          | –       |
/// | 1 Commit         | committed text | –    | –          | –       |
/// | 2 UpdatePreedit  | preedit text   | byte offset | –   | –       |
/// | 3 ClearPreedit   | –   | –             | –          | –       |
/// | 4 ShowCandidates | –   | –             | word list  | index   |
/// | 5 HideCandidates | –   | –             | –          | –       |
#[derive(Debug, Clone, PartialEq, Eq, Type, Serialize, Deserialize)]
pub struct IpcAction {
    pub kind: u8,
    pub text: String,
    pub cursor: u32,
    pub candidates: Vec<String>,
    pub focused: u32,
}

impl IpcAction {
    pub fn passthrough() -> Self {
        Self { kind: ACTION_PASSTHROUGH, text: String::new(), cursor: 0, candidates: vec![], focused: 0 }
    }

    pub fn commit(text: impl Into<String>) -> Self {
        Self { kind: ACTION_COMMIT, text: text.into(), cursor: 0, candidates: vec![], focused: 0 }
    }

    pub fn update_preedit(text: impl Into<String>, cursor: u32) -> Self {
        Self { kind: ACTION_UPDATE_PREEDIT, text: text.into(), cursor, candidates: vec![], focused: 0 }
    }

    pub fn clear_preedit() -> Self {
        Self { kind: ACTION_CLEAR_PREEDIT, text: String::new(), cursor: 0, candidates: vec![], focused: 0 }
    }

    pub fn show_candidates(candidates: Vec<String>, focused: u32) -> Self {
        Self { kind: ACTION_SHOW_CANDIDATES, text: String::new(), cursor: 0, candidates, focused }
    }

    pub fn hide_candidates() -> Self {
        Self { kind: ACTION_HIDE_CANDIDATES, text: String::new(), cursor: 0, candidates: vec![], focused: 0 }
    }

    pub fn update_status(indicator: impl Into<String>) -> Self {
        Self { kind: ACTION_UPDATE_STATUS, text: indicator.into(), cursor: 0, candidates: vec![], focused: 0 }
    }
}

// ── IpcKey ────────────────────────────────────────────────────────────────────

/// Key symbol constants used in `IpcKey::key_sym`.
///
/// Standard printable characters use their Unicode code point directly.
/// Special keys use X11 keysym values (0xFF__).
pub mod keysym {
    pub const SPACE: u32 = 0x0020;
    pub const RETURN: u32 = 0xFF0D;
    pub const BACKSPACE: u32 = 0xFF08;
    pub const TAB: u32 = 0xFF09;
    pub const ESCAPE: u32 = 0xFF1B;
    pub const DELETE: u32 = 0xFFFF;
    pub const LEFT: u32 = 0xFF51;
    pub const UP: u32 = 0xFF52;
    pub const RIGHT: u32 = 0xFF53;
    pub const DOWN: u32 = 0xFF54;
    pub const PAGE_UP: u32 = 0xFF55;
    pub const PAGE_DOWN: u32 = 0xFF56;
    /// F1–F15: F(n) = F1_BASE + (n - 1)
    pub const F1_BASE: u32 = 0xFFBE;
}

// ── InputMode ─────────────────────────────────────────────────────────────────

/// Input mode, used with `SetInputMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Type, Serialize, Deserialize)]
#[repr(u8)]
pub enum InputMode {
    Hiragana = 0,
    Katakana = 1,
    WideAscii = 2,
    Ascii = 3,
}
