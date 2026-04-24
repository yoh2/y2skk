/// Convert an evdev keycode to an X11 keysym for a US QWERTY layout.
///
/// Returns 0 for unknown or modifier-only keycodes.  Phase A only; a proper
/// xkbcommon-backed implementation can replace this later.
pub fn evdev_to_keysym(keycode: u32, shift: bool) -> u32 {
    match keycode {
        1  => 0xFF1B, // Escape
        2  => if shift { b'!' as u32 } else { b'1' as u32 },
        3  => if shift { b'@' as u32 } else { b'2' as u32 },
        4  => if shift { b'#' as u32 } else { b'3' as u32 },
        5  => if shift { b'$' as u32 } else { b'4' as u32 },
        6  => if shift { b'%' as u32 } else { b'5' as u32 },
        7  => if shift { b'^' as u32 } else { b'6' as u32 },
        8  => if shift { b'&' as u32 } else { b'7' as u32 },
        9  => if shift { b'*' as u32 } else { b'8' as u32 },
        10 => if shift { b'(' as u32 } else { b'9' as u32 },
        11 => if shift { b')' as u32 } else { b'0' as u32 },
        12 => if shift { b'_' as u32 } else { b'-' as u32 },
        13 => if shift { b'+' as u32 } else { b'=' as u32 },
        14 => 0xFF08, // Backspace
        15 => 0xFF09, // Tab
        16 => if shift { b'Q' as u32 } else { b'q' as u32 },
        17 => if shift { b'W' as u32 } else { b'w' as u32 },
        18 => if shift { b'E' as u32 } else { b'e' as u32 },
        19 => if shift { b'R' as u32 } else { b'r' as u32 },
        20 => if shift { b'T' as u32 } else { b't' as u32 },
        21 => if shift { b'Y' as u32 } else { b'y' as u32 },
        22 => if shift { b'U' as u32 } else { b'u' as u32 },
        23 => if shift { b'I' as u32 } else { b'i' as u32 },
        24 => if shift { b'O' as u32 } else { b'o' as u32 },
        25 => if shift { b'P' as u32 } else { b'p' as u32 },
        26 => if shift { b'{' as u32 } else { b'[' as u32 },
        27 => if shift { b'}' as u32 } else { b']' as u32 },
        28 | 96 => 0xFF0D, // Return / KP_Enter
        30 => if shift { b'A' as u32 } else { b'a' as u32 },
        31 => if shift { b'S' as u32 } else { b's' as u32 },
        32 => if shift { b'D' as u32 } else { b'd' as u32 },
        33 => if shift { b'F' as u32 } else { b'f' as u32 },
        34 => if shift { b'G' as u32 } else { b'g' as u32 },
        35 => if shift { b'H' as u32 } else { b'h' as u32 },
        36 => if shift { b'J' as u32 } else { b'j' as u32 },
        37 => if shift { b'K' as u32 } else { b'k' as u32 },
        38 => if shift { b'L' as u32 } else { b'l' as u32 },
        39 => if shift { b':' as u32 } else { b';' as u32 },
        40 => if shift { b'"' as u32 } else { b'\'' as u32 },
        41 => if shift { b'~' as u32 } else { b'`' as u32 },
        43 => if shift { b'|' as u32 } else { b'\\' as u32 },
        44 => if shift { b'Z' as u32 } else { b'z' as u32 },
        45 => if shift { b'X' as u32 } else { b'x' as u32 },
        46 => if shift { b'C' as u32 } else { b'c' as u32 },
        47 => if shift { b'V' as u32 } else { b'v' as u32 },
        48 => if shift { b'B' as u32 } else { b'b' as u32 },
        49 => if shift { b'N' as u32 } else { b'n' as u32 },
        50 => if shift { b'M' as u32 } else { b'm' as u32 },
        51 => if shift { b'<' as u32 } else { b',' as u32 },
        52 => if shift { b'>' as u32 } else { b'.' as u32 },
        53 => if shift { b'?' as u32 } else { b'/' as u32 },
        57 => 0x0020, // Space
        // Navigation
        102 => 0xFF50, // Home
        103 => 0xFF52, // Up
        104 => 0xFF55, // Page Up
        105 => 0xFF51, // Left
        106 => 0xFF53, // Right
        107 => 0xFF57, // End
        108 => 0xFF54, // Down
        109 => 0xFF56, // Page Down
        110 => 0xFF63, // Insert
        111 => 0xFFFF, // Delete
        // Function keys F1–F12
        59  => skk_ipc::keysym::F1_BASE,
        60  => skk_ipc::keysym::F1_BASE + 1,
        61  => skk_ipc::keysym::F1_BASE + 2,
        62  => skk_ipc::keysym::F1_BASE + 3,
        63  => skk_ipc::keysym::F1_BASE + 4,
        64  => skk_ipc::keysym::F1_BASE + 5,
        65  => skk_ipc::keysym::F1_BASE + 6,
        66  => skk_ipc::keysym::F1_BASE + 7,
        67  => skk_ipc::keysym::F1_BASE + 8,
        68  => skk_ipc::keysym::F1_BASE + 9,
        87  => skk_ipc::keysym::F1_BASE + 10, // F11
        88  => skk_ipc::keysym::F1_BASE + 11, // F12
        // Modifier keycodes return 0 — handled via the modifiers event
        29 | 97  => 0, // Left/Right Ctrl
        42 | 54  => 0, // Left/Right Shift
        56 | 100 => 0, // Left/Right Alt
        125 | 126 => 0, // Left/Right Super (Meta)
        _ => 0,
    }
}

/// Convert the `mods_depressed` + `mods_latched` bitmask from a
/// `wl_keyboard::modifiers` event to the skk-ipc modifier bitmask.
///
/// Assumes the standard XKB modifier layout for Linux
/// (Shift=bit 0, Control=bit 2, Mod1/Alt=bit 3, Mod4/Super=bit 6).
pub fn xkb_mods_to_skk(mods_depressed: u32, mods_latched: u32) -> u32 {
    let combined = mods_depressed | mods_latched;
    let mut result = 0u32;
    if combined & (1 << 0) != 0 { result |= 0x01; } // Shift
    if combined & (1 << 2) != 0 { result |= 0x04; } // Control
    if combined & (1 << 3) != 0 { result |= 0x08; } // Alt (Mod1)
    if combined & (1 << 6) != 0 { result |= 0x40; } // Super (Mod4)
    result
}
