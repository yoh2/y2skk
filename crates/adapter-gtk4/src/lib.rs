use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

// ── GIO module entry points (called by GTK4 via dlsym) ───────────────────────
//
// Rust's cdylib linker version script only exports #[no_mangle] pub extern "C"
// symbols.  The actual GObject/GTK logic lives in the C shim (im_module.c) under
// _y2skk_g_io_module_* names; these thin Rust wrappers give them the required
// public names that GTK4's GIO module loader looks for.

extern "C" {
    fn _y2skk_g_io_module_load(module: *mut c_void);
    fn _y2skk_g_io_module_unload(module: *mut c_void);
    fn _y2skk_g_io_module_query() -> *mut *mut c_char;
}

/// GIO module load entry point. Called by GTK4 once when the shared object
/// is dlopen()'d from `$LIBDIR/gtk-4.0/immodules/`.
///
/// # Safety
///
/// Called by GLib/GIO with a valid `GIOModule*` pointer.
#[no_mangle]
pub unsafe extern "C" fn g_io_module_load(module: *mut c_void) {
    _y2skk_g_io_module_load(module);
}

/// GIO module unload entry point. Called when the module is unloaded.
///
/// # Safety
///
/// Called by GLib/GIO with the same `GIOModule*` previously passed to load.
#[no_mangle]
pub unsafe extern "C" fn g_io_module_unload(module: *mut c_void) {
    _y2skk_g_io_module_unload(module);
}

/// GIO module query entry point. Returns a NULL-terminated array of
/// extension point names this module implements (just `gtk-im-module`).
/// The returned vector is owned by the caller and freed with `g_strfreev`.
#[no_mangle]
pub extern "C" fn g_io_module_query() -> *mut *mut c_char {
    unsafe { _y2skk_g_io_module_query() }
}

use skk_ipc::dispatch::{dispatch as dispatch_actions, ActionSink};
use skk_ipc::proxy::reconnect::ReconnectingClient;
use skk_ipc::NO_GHOST;

// ── Callbacks struct (mirrors y2skk_im.h) ────────────────────────────────────

/// Function pointer table passed by the C shim for dispatching engine actions.
#[repr(C)]
pub struct Y2skkCallbacks {
    pub commit: unsafe extern "C" fn(*mut c_void, *const c_char),
    pub update_preedit: unsafe extern "C" fn(*mut c_void, *const c_char, c_uint, c_uint),
    pub clear_preedit: unsafe extern "C" fn(*mut c_void),
    pub show_candidates:
        unsafe extern "C" fn(*mut c_void, *const *const c_char, c_uint, *const c_char),
    pub hide_candidates: unsafe extern "C" fn(*mut c_void),
    pub update_status: unsafe extern "C" fn(*mut c_void, *const c_char, c_uint),
}

// ── ActionSink adapter for the GTK4 callback table ───────────────────────────

struct CallbackSink {
    ctx: *mut c_void,
    cbs: *const Y2skkCallbacks,
}

impl ActionSink for CallbackSink {
    fn commit(&mut self, text: &str) {
        if let Ok(cs) = CString::new(text) {
            unsafe { ((*self.cbs).commit)(self.ctx, cs.as_ptr()) }
        }
    }

    fn update_preedit(&mut self, text: &str, cursor: u32, ghost_start: Option<u32>) {
        if let Ok(cs) = CString::new(text) {
            let ghost = ghost_start.unwrap_or(NO_GHOST);
            unsafe { ((*self.cbs).update_preedit)(self.ctx, cs.as_ptr(), cursor, ghost) }
        }
    }

    fn clear_preedit(&mut self) {
        unsafe { ((*self.cbs).clear_preedit)(self.ctx) }
    }

    fn show_candidates(&mut self, candidates: &[String], focused: u32, sel_keys: &str) {
        let cstrings: Vec<CString> = candidates
            .iter()
            .filter_map(|w| CString::new(w.as_str()).ok())
            .collect();
        let ptrs: Vec<*const c_char> = cstrings
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let keys_cs = CString::new(sel_keys).unwrap_or_default();
        unsafe { ((*self.cbs).show_candidates)(self.ctx, ptrs.as_ptr(), focused, keys_cs.as_ptr()) }
    }

    fn hide_candidates(&mut self) {
        unsafe { ((*self.cbs).hide_candidates)(self.ctx) }
    }

    fn update_status(&mut self, indicator: &str, timeout_ms: u32) {
        if let Ok(cs) = CString::new(indicator) {
            // timeout_ms is passed via action.cursor in the GTK4 C callback.
            unsafe { ((*self.cbs).update_status)(self.ctx, cs.as_ptr(), timeout_ms) }
        }
    }
}

// ── Global reconnecting client ────────────────────────────────────────────────

static CLIENT: OnceLock<ReconnectingClient> = OnceLock::new();

fn client() -> Option<&'static ReconnectingClient> {
    CLIENT.get()
}

// ── Exported Rust functions (called from C shim) ──────────────────────────────

/// Called once when the GTK module is loaded. Establishes the D-Bus connection.
#[no_mangle]
pub extern "C" fn y2skk_init() -> c_int {
    match ReconnectingClient::new() {
        Ok(c) => {
            let _ = CLIENT.set(c);
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

/// Allocates a new IME session and returns a local handle (0 on failure).
///
/// # Safety
///
/// `app_id` must be a valid C string or NULL.
#[no_mangle]
pub unsafe extern "C" fn y2skk_create_session(app_id: *const c_char) -> c_uint {
    let label = if app_id.is_null() {
        "gtk4".to_string()
    } else {
        CStr::from_ptr(app_id).to_string_lossy().into_owned()
    };

    match client() {
        Some(c) => c.create_handle(&label),
        None => {
            eprintln!("y2skk: create_session called before init");
            0
        }
    }
}

/// Releases an IME session.
#[no_mangle]
pub extern "C" fn y2skk_destroy_session(handle: c_uint) {
    if let Some(c) = client() {
        c.destroy_handle(handle);
    }
}

/// Processes a key event, dispatching engine actions via `cbs`.
/// Returns 1 if the key was consumed by the IME, 0 for passthrough.
///
/// # Safety
///
/// `cbs` must be a valid pointer
#[no_mangle]
pub unsafe extern "C" fn y2skk_process_key(
    handle: c_uint,
    keyval: c_uint,
    modifiers: c_uint,
    is_press: c_int,
    ctx: *mut c_void,
    cbs: *const Y2skkCallbacks,
) -> c_int {
    let Some(c) = client() else { return 0 };

    let actions = match c.process_key(handle, keyval, modifiers, is_press != 0) {
        Ok(a) => a,
        Err(_) => return 0,
    };

    let mut sink = CallbackSink { ctx, cbs };
    let result = dispatch_actions(&actions, &mut sink);
    if result.consumed && !result.force_passthrough {
        1
    } else {
        0
    }
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
