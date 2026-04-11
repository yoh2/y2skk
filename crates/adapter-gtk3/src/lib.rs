mod proxy;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

// ── GTK3 IM module entry points ───────────────────────────────────────────────
//
// Rust's cdylib linker version script only exports #[no_mangle] pub extern "C"
// symbols.  The actual GObject/GTK logic lives in the C shim (im_module.c) under
// _y2skk_im_module_* names; these thin Rust wrappers give them the required
// public names.

extern "C" {
    fn _y2skk_im_module_list(contexts: *mut *const *const c_void, n_contexts: *mut c_int);
    fn _y2skk_im_module_init(module: *mut c_void);
    fn _y2skk_im_module_exit();
    fn _y2skk_im_module_create(context_id: *const c_char) -> *mut c_void;
}

#[no_mangle]
pub unsafe extern "C" fn im_module_list(
    contexts: *mut *const *const c_void,
    n_contexts: *mut c_int,
) {
    _y2skk_im_module_list(contexts, n_contexts);
}

#[no_mangle]
pub unsafe extern "C" fn im_module_init(module: *mut c_void) {
    _y2skk_im_module_init(module);
}

#[no_mangle]
pub extern "C" fn im_module_exit() {
    unsafe { _y2skk_im_module_exit() };
}

#[no_mangle]
pub unsafe extern "C" fn im_module_create(context_id: *const c_char) -> *mut c_void {
    _y2skk_im_module_create(context_id)
}

use proxy::DaemonProxy;
use skk_ipc::{
    ACTION_CLEAR_PREEDIT, ACTION_COMMIT, ACTION_HIDE_CANDIDATES, ACTION_PASSTHROUGH,
    ACTION_SHOW_CANDIDATES, ACTION_UPDATE_PREEDIT, ACTION_UPDATE_STATUS,
};

// ── Callbacks struct (mirrors y2skk_im.h) ────────────────────────────────────

/// Function pointer table passed by the C shim for dispatching engine actions.
#[repr(C)]
pub struct Y2skkCallbacks {
    pub commit: unsafe extern "C" fn(*mut c_void, *const c_char),
    pub update_preedit: unsafe extern "C" fn(*mut c_void, *const c_char, c_uint),
    pub clear_preedit: unsafe extern "C" fn(*mut c_void),
    pub show_candidates: unsafe extern "C" fn(*mut c_void, *const *const c_char, c_uint, *const c_char),
    pub hide_candidates: unsafe extern "C" fn(*mut c_void),
    pub update_status: unsafe extern "C" fn(*mut c_void, *const c_char, c_uint),
}

// ── Global D-Bus proxy ────────────────────────────────────────────────────────

static PROXY: OnceLock<DaemonProxy> = OnceLock::new();

fn proxy() -> Option<&'static DaemonProxy> {
    PROXY.get()
}

// ── Exported Rust functions (called from C shim) ──────────────────────────────

/// Called once when the GTK module is loaded. Establishes the D-Bus connection.
#[no_mangle]
pub extern "C" fn y2skk_init() -> c_int {
    match DaemonProxy::connect() {
        Ok(p) => {
            let _ = PROXY.set(p);
            0
        }
        Err(e) => {
            eprintln!("y2skk: D-Bus connection failed: {e}");
            1
        }
    }
}

/// Called once when the GTK module is unloaded.
#[no_mangle]
pub extern "C" fn y2skk_fini() {
    // OnceLock cannot be cleared, but the connection will be dropped with the process.
}

/// Allocates a new IME session and returns its ID (0 on failure).
#[no_mangle]
pub extern "C" fn y2skk_create_session(app_id: *const c_char) -> c_uint {
    let label = if app_id.is_null() {
        "gtk3".to_string()
    } else {
        unsafe { CStr::from_ptr(app_id).to_string_lossy().into_owned() }
    };

    match proxy().and_then(|p| p.create_session(&label).ok()) {
        Some(id) => id,
        None => {
            eprintln!("y2skk: create_session failed");
            0
        }
    }
}

/// Releases an IME session.
#[no_mangle]
pub extern "C" fn y2skk_destroy_session(session_id: c_uint) {
    if let Some(p) = proxy() {
        let _ = p.destroy_session(session_id);
    }
}

/// Processes a key event, dispatching engine actions via `cbs`.
/// Returns 1 if the key was consumed by the IME, 0 for passthrough.
#[no_mangle]
pub unsafe extern "C" fn y2skk_process_key(
    session_id: c_uint,
    keyval: c_uint,
    modifiers: c_uint,
    is_press: c_int,
    ctx: *mut c_void,
    cbs: *const Y2skkCallbacks,
) -> c_int {
    let Some(p) = proxy() else { return 0 };
    let cbs = &*cbs;

    let actions = match p.process_key(session_id, keyval, modifiers, is_press != 0) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("y2skk: process_key D-Bus error: {e}");
            return 0;
        }
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
                // Build a NULL-terminated array of C strings
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
                if let Ok(cs) = CString::new(action.text.as_str()) {
                    (cbs.update_status)(ctx, cs.as_ptr(), action.cursor);
                }
            }
            _ => {}
        }
    }

    if consumed && !force_passthrough { 1 } else { 0 }
}

/// Notifies the daemon that the context gained focus.
#[no_mangle]
pub extern "C" fn y2skk_focus_in(session_id: c_uint) {
    // Future: send focus event to daemon for mode persistence
    let _ = session_id;
}

/// Notifies the daemon that the context lost focus.
#[no_mangle]
pub extern "C" fn y2skk_focus_out(session_id: c_uint) {
    let _ = session_id;
}

/// Resets the IME state for the session.
#[no_mangle]
pub extern "C" fn y2skk_reset(session_id: c_uint) {
    // Future: call a Reset D-Bus method
    let _ = session_id;
}
