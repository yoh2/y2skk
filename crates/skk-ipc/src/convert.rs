//! Conversions between `skk-core` types and IPC types.

use skk_core::engine::{EngineAction, Preedit};
use skk_core::key::{Key, KeyEvent, Modifiers};

use crate::{IpcAction, keysym};

// ── EngineAction → IpcAction ──────────────────────────────────────────────────

impl From<EngineAction> for IpcAction {
    fn from(action: EngineAction) -> Self {
        match action {
            EngineAction::Commit(text) => IpcAction::commit(text),
            EngineAction::UpdatePreedit(Preedit { text, cursor }) => {
                IpcAction::update_preedit(text, cursor as u32)
            }
            EngineAction::ClearPreedit => IpcAction::clear_preedit(),
            EngineAction::ShowCandidates(candidates, focused, sel_keys) => {
                // Include annotation with ';' separator when present (standard SKK convention).
                let words = candidates
                    .into_iter()
                    .map(|c| match c.annotation {
                        Some(ann) => format!("{};{}", c.word, ann),
                        None => c.word,
                    })
                    .collect();
                // Use the `text` field (otherwise unused for ShowCandidates) to carry
                // the selection key characters so the UI can display labels like "a:候補".
                let mut action = IpcAction::show_candidates(words, focused as u32);
                action.text = sel_keys;
                action
            }
            EngineAction::HideCandidates => IpcAction::hide_candidates(),
            EngineAction::UpdateStatus(indicator) => IpcAction::update_status(indicator),
            EngineAction::Passthrough => IpcAction::passthrough(),
        }
    }
}

// ── (key_sym, modifiers, is_press) → KeyEvent ────────────────────────────────

/// Converts a raw D-Bus key event (X11 keysym + modifier bitmask + press flag)
/// into a `skk-core` `KeyEvent`.
///
/// `modifiers_raw` bit layout (matches X11 / GDK modifier mask):
/// - bit 0 (0x01): Shift
/// - bit 2 (0x04): Control
/// - bit 3 (0x08): Alt/Mod1
/// - bit 6 (0x40): Meta/Super
pub fn key_event_from_raw(key_sym: u32, modifiers_raw: u32, is_press: bool) -> KeyEvent {
    let key = keysym_to_key(key_sym);
    let modifiers = modifiers_from_raw(modifiers_raw);
    if is_press {
        KeyEvent::press(key, modifiers)
    } else {
        KeyEvent::release(key, modifiers)
    }
}

fn keysym_to_key(sym: u32) -> Key {
    match sym {
        keysym::SPACE => Key::Space,
        keysym::RETURN => Key::Return,
        keysym::BACKSPACE => Key::BackSpace,
        keysym::TAB => Key::Tab,
        keysym::ESCAPE => Key::Escape,
        keysym::DELETE => Key::Delete,
        keysym::LEFT => Key::Left,
        keysym::UP => Key::Up,
        keysym::RIGHT => Key::Right,
        keysym::DOWN => Key::Down,
        keysym::PAGE_UP => Key::PageUp,
        keysym::PAGE_DOWN => Key::PageDown,
        f if (keysym::F1_BASE..keysym::F1_BASE + 15).contains(&f) => {
            Key::F((f - keysym::F1_BASE + 1) as u8)
        }
        // X11 Unicode keysyms: 0x01000000 | codepoint (used by GDK for composed chars)
        c if c & 0xFF00_0000 == 0x0100_0000 => {
            if let Some(ch) = char::from_u32(c & 0x00FF_FFFF) {
                return Key::Char(ch);
            }
            Key::Other(c)
        }
        // Printable Unicode character (direct keysym = codepoint).
        // Exclude 0xFF00–0xFFFF: these are X11 special keysyms (modifier keys,
        // function keys, etc.) and must NOT be treated as Unicode characters.
        // e.g. GDK_KEY_Shift_L = 0xFFE1, GDK_KEY_Shift_R = 0xFFE2.
        c if (0x20..=0xFEFF).contains(&c) || (0x1_0000..=0x10_FFFF).contains(&c) => {
            if let Some(ch) = char::from_u32(c) {
                return Key::Char(ch);
            }
            Key::Other(c)
        }
        other => Key::Other(other),
    }
}

fn modifiers_from_raw(raw: u32) -> Modifiers {
    let mut m = Modifiers::empty();
    if raw & 0x01 != 0 { m |= Modifiers::SHIFT; }
    if raw & 0x04 != 0 { m |= Modifiers::CTRL; }
    if raw & 0x08 != 0 { m |= Modifiers::ALT; }
    if raw & 0x40 != 0 { m |= Modifiers::META; }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keysym_ascii() {
        let ev = key_event_from_raw('a' as u32, 0, true);
        assert_eq!(ev.key, Key::Char('a'));
        assert_eq!(ev.modifiers, Modifiers::empty());
        assert!(ev.is_press);
    }

    #[test]
    fn test_keysym_return() {
        let ev = key_event_from_raw(keysym::RETURN, 0, true);
        assert_eq!(ev.key, Key::Return);
    }

    #[test]
    fn test_keysym_ctrl_j() {
        let ev = key_event_from_raw('j' as u32, 0x04, true);
        assert_eq!(ev.key, Key::Char('j'));
        assert_eq!(ev.modifiers, Modifiers::CTRL);
    }

    #[test]
    fn test_engine_action_commit() {
        let action = EngineAction::Commit("か".into());
        let ipc: IpcAction = action.into();
        assert_eq!(ipc.kind, crate::ACTION_COMMIT);
        assert_eq!(ipc.text, "か");
    }
}
