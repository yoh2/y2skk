use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

use skk_ipc::proxy::reconnect::ReconnectingClient;
use skk_ipc::{
    ACTION_CLEAR_PREEDIT, ACTION_COMMIT, ACTION_HIDE_CANDIDATES, ACTION_PASSTHROUGH,
    ACTION_SHOW_CANDIDATES, ACTION_UPDATE_PREEDIT, ACTION_UPDATE_STATUS,
};

// ── Callbacks struct (mirrors rust_bridge.h) ──────────────────────────────────

/// Function pointer table passed by the C++ shim for dispatching engine actions.
#[repr(C)]
pub struct Y2skkCallbacks {
    pub commit: unsafe extern "C" fn(*mut c_void, *const c_char),
    pub update_preedit: unsafe extern "C" fn(*mut c_void, *const c_char, c_uint),
    pub clear_preedit: unsafe extern "C" fn(*mut c_void),
    pub show_candidates: unsafe extern "C" fn(*mut c_void, *const *const c_char, c_uint, *const c_char),
    pub hide_candidates: unsafe extern "C" fn(*mut c_void),
    pub update_status: unsafe extern "C" fn(*mut c_void, *const c_char),
}

// ── Global D-Bus proxy ────────────────────────────────────────────────────────

static CLIENT: OnceLock<ReconnectingClient> = OnceLock::new();

fn client() -> Option<&'static ReconnectingClient> {
    CLIENT.get()
}

// ── Exported functions (called from C++ shim) ─────────────────────────────────

#[no_mangle]
pub extern "C" fn y2skk_init() -> c_int {
    match ReconnectingClient::new() {
        Ok(c) => {
            let _ = CLIENT.set(c);
            0
        }
        Err(e) => {
            eprintln!("y2skk-qt6: D-Bus connection failed: {e}");
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn y2skk_fini() {}

#[no_mangle]
pub extern "C" fn y2skk_create_session(app_id: *const c_char) -> c_uint {
    let label = if app_id.is_null() {
        "qt6".to_string()
    } else {
        unsafe { CStr::from_ptr(app_id).to_string_lossy().into_owned() }
    };
    match client() {
        Some(c) => c.create_handle(&label),
        None => {
            eprintln!("y2skk-qt6: create_session called before init");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn y2skk_destroy_session(handle: c_uint) {
    if let Some(c) = client() {
        c.destroy_handle(handle);
    }
}

/// Processes a key event and dispatches engine actions via `cbs`.
/// Returns 1 if consumed, 0 for passthrough.
///
/// Qt key modifiers bitmask:
///   0x02000000 = Shift, 0x04000000 = Ctrl, 0x08000000 = Alt, 0x10000000 = Meta
#[no_mangle]
pub unsafe extern "C" fn y2skk_process_key(
    handle: c_uint,
    key_sym: c_uint,
    modifiers: c_uint,
    is_press: c_int,
    ctx: *mut c_void,
    cbs: *const Y2skkCallbacks,
) -> c_int {
    let Some(c) = client() else { return 0 };
    let cbs = &*cbs;

    // Convert Qt modifiers to X11 modifier bitmask expected by skk-ipc.
    let x11_mods = qt_mods_to_x11(modifiers);
    // Convert Qt::Key + modifiers to X11 keysym.
    // Modifiers are needed to determine case: Qt::Key is always uppercase for letters.
    let keysym = qt_key_to_keysym(key_sym, modifiers);

    let actions = match c.process_key(handle, keysym, x11_mods, is_press != 0) {
        Ok(a) => a,
        Err(_) => return 0,
    };

    let mut consumed = false;
    let mut force_passthrough = false;
    for action in &actions {
        match action.kind {
            k if k == ACTION_PASSTHROUGH => {
                // Passthrough can appear alongside commit actions (e.g. Return confirms
                // the candidate AND sends Enter to the application).
                force_passthrough = true;
            }
            k if k == ACTION_COMMIT => {
                consumed = true;
                if let Ok(cs) = CString::new(action.text.as_str()) {
                    (cbs.commit)(ctx, cs.as_ptr());
                }
            }
            k if k == ACTION_UPDATE_PREEDIT => {
                consumed = true;
                if let Ok(cs) = CString::new(action.text.as_str()) {
                    (cbs.update_preedit)(ctx, cs.as_ptr(), action.cursor);
                }
            }
            k if k == ACTION_CLEAR_PREEDIT => {
                consumed = true;
                (cbs.clear_preedit)(ctx);
            }
            k if k == ACTION_SHOW_CANDIDATES => {
                consumed = true;
                let cstrings: Vec<CString> = action
                    .candidates
                    .iter()
                    .filter_map(|w| CString::new(w.as_str()).ok())
                    .collect();
                let ptrs: Vec<*const c_char> = cstrings
                    .iter()
                    .map(|s| s.as_ptr())
                    .chain(std::iter::once(std::ptr::null()))
                    .collect();
                let keys_cs = CString::new(action.text.as_str()).unwrap_or_default();
                (cbs.show_candidates)(ctx, ptrs.as_ptr(), action.focused, keys_cs.as_ptr());
            }
            k if k == ACTION_HIDE_CANDIDATES => {
                consumed = true;
                (cbs.hide_candidates)(ctx);
            }
            k if k == ACTION_UPDATE_STATUS => {
                consumed = true;
                if let Ok(cs) = CString::new(action.text.as_str()) {
                    (cbs.update_status)(ctx, cs.as_ptr());
                }
            }
            _ => {}
        }
    }

    if consumed && !force_passthrough { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn y2skk_focus_in(session_id: c_uint) {
    let _ = session_id;
}

#[no_mangle]
pub extern "C" fn y2skk_focus_out(session_id: c_uint) {
    let _ = session_id;
}

#[no_mangle]
pub extern "C" fn y2skk_reset(session_id: c_uint) {
    let _ = session_id;
}

// ── Key translation ───────────────────────────────────────────────────────────

/// Converts Qt::KeyboardModifiers to the X11 modifier bitmask used by skk-ipc.
fn qt_mods_to_x11(qt_mods: u32) -> u32 {
    let mut x11 = 0u32;
    if qt_mods & 0x02000000 != 0 { x11 |= 0x01; } // Shift
    if qt_mods & 0x04000000 != 0 { x11 |= 0x04; } // Ctrl
    if qt_mods & 0x08000000 != 0 { x11 |= 0x08; } // Alt
    if qt_mods & 0x10000000 != 0 { x11 |= 0x40; } // Meta
    x11
}

/// Converts a Qt::Key value and Qt::KeyboardModifiers to an X11 keysym.
///
/// Qt::Key for letter keys is always the uppercase value (e.g. Qt::Key_A = 0x41)
/// regardless of whether Shift is held.  We must check the Shift modifier bit
/// and emit a lowercase keysym when Shift is absent, because the SKK engine
/// distinguishes uppercase (conversion trigger) from lowercase (kana input).
fn qt_key_to_keysym(qt_key: u32, qt_mods: u32) -> u32 {
    let shift = qt_mods & 0x02000000 != 0; // Qt::ShiftModifier
    use skk_ipc::keysym;
    match qt_key {
        0x01000000 => keysym::ESCAPE,
        0x01000001 => keysym::TAB,
        0x01000004 | 0x01000005 => keysym::RETURN, // Key_Return and Key_Enter (numpad)
        0x01000003 => keysym::BACKSPACE,
        0x01000007 => keysym::DELETE,
        0x01000012 => keysym::LEFT,
        0x01000013 => keysym::UP,
        0x01000014 => keysym::RIGHT,
        0x01000015 => keysym::DOWN,
        0x01000016 => keysym::PAGE_UP,
        0x01000017 => keysym::PAGE_DOWN,
        // Modifier / lock keys.  Qt uses the 0x0100____ prefix which
        // coincidentally matches the X11 Unicode-keysym prefix, so passing
        // these through unchanged would make the daemon decode e.g.
        // Key_Shift (0x01000020) as the Unicode code point 0x20 (= SPACE).
        // Map them to real X11 modifier keysyms instead.
        0x01000020 => 0xFFE1, // Key_Shift      → XK_Shift_L
        0x01000021 => 0xFFE3, // Key_Control    → XK_Control_L
        0x01000022 => 0xFFE7, // Key_Meta       → XK_Meta_L
        0x01000023 => 0xFFE9, // Key_Alt        → XK_Alt_L
        0x01000024 => 0xFFE5, // Key_CapsLock   → XK_Caps_Lock
        0x01000025 => 0xFF7F, // Key_NumLock    → XK_Num_Lock
        0x01000026 => 0xFF14, // Key_ScrollLock → XK_Scroll_Lock
        0x01000053 => 0xFFEB, // Key_Super_L    → XK_Super_L
        0x01000054 => 0xFFEC, // Key_Super_R    → XK_Super_R
        0x01001103 => 0xFE03, // Key_AltGr      → XK_ISO_Level3_Shift
        // F1–F12: Qt uses 0x01000030–0x0100003B
        f @ 0x01000030..=0x0100003B => keysym::F1_BASE + (f - 0x01000030),
        // Space
        0x20 => keysym::SPACE,
        // Letter keys A–Z: Qt::Key is always uppercase (0x41–0x5A).
        // Emit lowercase keysym when Shift is not held (same convention as X11).
        c @ 0x41..=0x5A if !shift => c + 0x20,
        // Printable Unicode: Qt::Key == Unicode code point == X11 keysym
        c => c,
    }
}
