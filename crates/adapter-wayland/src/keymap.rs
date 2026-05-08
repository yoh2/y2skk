use std::os::fd::OwnedFd;
use xkbcommon::xkb;

/// XKB keymap + state derived from the keymap fd received via wl_keyboard.keymap.
pub struct XkbState {
    state: xkb::State,
}

impl XkbState {
    /// Build from the memfd sent by the compositor in wl_keyboard.keymap.
    pub fn from_fd(fd: OwnedFd, size: u32) -> anyhow::Result<Self> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // SAFETY: fd is a valid memfd from the compositor; size matches its content length.
        let keymap = unsafe {
            xkb::Keymap::new_from_fd(
                &context,
                fd,
                size as usize,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        }
        .map_err(|e| anyhow::anyhow!("keymap fd read error: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("failed to compile XKB keymap"))?;

        Ok(Self {
            state: xkb::State::new(&keymap),
        })
    }

    /// Sync modifier state using values from wl_keyboard.modifiers.
    pub fn update_mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        self.state
            .update_mask(depressed, latched, locked, 0, 0, group);
    }

    /// Translate an evdev keycode to an X11-compatible keysym using the current
    /// modifier state, or 0 (XKB_KEY_NoSymbol) when there is no mapping.
    pub fn key_get_one_sym(&self, evdev_keycode: u32) -> u32 {
        // XKB keycodes are evdev + 8.
        let keycode: xkb::Keycode = (evdev_keycode + 8).into();
        self.state.key_get_one_sym(keycode).raw()
    }

    /// Returns true if the keymap says this key should participate in key repeat
    /// (typically false for modifier keys).
    pub fn key_repeats(&self, evdev_keycode: u32) -> bool {
        let keycode: xkb::Keycode = (evdev_keycode + 8).into();
        self.state.get_keymap().key_repeats(keycode)
    }
}

/// Convert the `mods_depressed` + `mods_latched` bitmask from a
/// `wl_keyboard::modifiers` event to the skk-ipc modifier bitmask.
///
/// The compositor already applies xkb processing (including key remappings like
/// Ctrl↔CapsLock), so these bits reflect the logical modifier state.
/// Assumes the standard XKB modifier layout for Linux
/// (Shift=bit 0, Control=bit 2, Mod1/Alt=bit 3, Mod4/Super=bit 6).
pub fn xkb_mods_to_skk(mods_depressed: u32, mods_latched: u32) -> u32 {
    let combined = mods_depressed | mods_latched;
    let mut result = 0u32;
    if combined & (1 << 0) != 0 {
        result |= 0x01;
    } // Shift
    if combined & (1 << 2) != 0 {
        result |= 0x04;
    } // Control
    if combined & (1 << 3) != 0 {
        result |= 0x08;
    } // Alt (Mod1)
    if combined & (1 << 6) != 0 {
        result |= 0x40;
    } // Super (Mod4)
    result
}
