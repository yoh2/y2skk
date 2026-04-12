//! X11 keycode → keysym conversion and modifier normalisation.
//!
//! The X server's keyboard mapping is fetched once at startup and cached.
//! [`resolve_keysym`] translates a raw hardware keycode + modifier state into
//! an X11 keysym, stripping lock/numlock bits so the daemon sees a clean
//! (keysym, modifier, is_press) triple identical in spirit to the one
//! produced by the GTK3 and Qt6 adapters.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, Keysym, Setup};

/// Cached keyboard mapping obtained from `GetKeyboardMapping`.
pub struct KeyMap {
    min_keycode: u8,
    keysyms_per_keycode: u8,
    keysyms: Vec<Keysym>,
}

// Modifier mask bits (X11 protocol).
const SHIFT_MASK: u16 = 1 << 0;
const LOCK_MASK: u16 = 1 << 1;
const MOD2_MASK: u16 = 1 << 4; // typically NumLock

impl KeyMap {
    /// Fetches the full keyboard mapping from the X server.
    pub fn new<C: Connection>(conn: &C, setup: &Setup) -> anyhow::Result<Self> {
        let min = setup.min_keycode;
        let max = setup.max_keycode;
        let count = max - min + 1;
        let reply = conn.get_keyboard_mapping(min, count)?.reply()?;
        Ok(Self {
            min_keycode: min,
            keysyms_per_keycode: reply.keysyms_per_keycode,
            keysyms: reply.keysyms,
        })
    }

    /// Resolves a keycode + modifier state into (keysym, cleaned_modifiers).
    ///
    /// The returned modifier mask has Lock (bit 1) and Mod2/NumLock (bit 4)
    /// stripped, matching the convention used by skk-daemon.
    pub fn resolve(&self, keycode: u8, state: u16) -> (Keysym, u32) {
        let keysym = self.lookup(keycode, state);
        // Strip Lock and Mod2 (NumLock) from the modifier mask the daemon sees.
        let clean = state & !(LOCK_MASK | MOD2_MASK);
        (keysym, clean as u32)
    }

    /// Core lookup: keycode + state → keysym, following the simplified
    /// algorithm in the X11 protocol spec (section 5, "Keyboards").
    fn lookup(&self, keycode: u8, state: u16) -> Keysym {
        if keycode < self.min_keycode {
            return 0; // NoSymbol
        }
        let idx = (keycode - self.min_keycode) as usize;
        let width = self.keysyms_per_keycode as usize;
        let base = idx * width;
        if base >= self.keysyms.len() {
            return 0;
        }

        let sym = |col: usize| -> Keysym {
            if base + col < self.keysyms.len() {
                self.keysyms[base + col]
            } else {
                0
            }
        };

        // Group 1 columns 0..1 (we ignore groups 2+ for MVP).
        let mut lower = sym(0);
        let mut upper = sym(1);

        // If the second keysym is NoSymbol, infer upper/lower from the first.
        if upper == 0 {
            // If the keysym is a Latin-1 letter, use the standard case pair.
            let (lo, hi) = keysym_case(lower);
            lower = lo;
            upper = hi;
        }

        let shifted = state & SHIFT_MASK != 0;
        let caps = state & LOCK_MASK != 0;

        match (shifted, caps) {
            (false, false) => lower,
            (false, true) => {
                // CapsLock: upper-case if the keysym is alphabetic, else lower.
                if is_alphabetic_keysym(lower) {
                    upper
                } else {
                    lower
                }
            }
            (true, false) => upper,
            (true, true) => {
                // Shift + CapsLock: for letters, shift cancels caps → lower.
                if is_alphabetic_keysym(lower) {
                    lower
                } else {
                    upper
                }
            }
        }
    }
}

/// Returns (lowercase, uppercase) keysym pair for a given keysym.
/// Only handles Latin-1 range; everything else returns (sym, sym).
fn keysym_case(sym: Keysym) -> (Keysym, Keysym) {
    // ASCII letters
    if (0x0041..=0x005A).contains(&sym) {
        // Already uppercase
        return (sym + 0x20, sym);
    }
    if (0x0061..=0x007A).contains(&sym) {
        // Already lowercase
        return (sym, sym - 0x20);
    }
    (sym, sym)
}

/// Returns true if the keysym has distinct upper/lower case forms.
fn is_alphabetic_keysym(sym: Keysym) -> bool {
    let (lo, hi) = keysym_case(sym);
    lo != hi
}
