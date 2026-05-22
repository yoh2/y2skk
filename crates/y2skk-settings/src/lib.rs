//! Rust core for the y2skk Qt6 settings GUI.
//!
//! The thin C++ Qt Widgets front-end calls these `extern "C"` functions; all
//! config schema knowledge, validation, comment-preserving write-back and the
//! D-Bus reload live here so they stay single-sourced with the daemon
//! (`skk-config`).  Config is marshalled across the FFI boundary as JSON.

use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};

use skk_config::{default_config_path, kana_table_search_dirs, validate, Config};
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

// ── Internal logic (no FFI; unit-testable) ─────────────────────────────────

/// Loads the config at `path` *without* tilde-normalisation so the GUI shows and
/// round-trips the file's raw values (e.g. keeps `~/...`).  Returns defaults if
/// the file is absent or unparseable.
fn load_config_raw(path: &Path) -> Config {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str::<Config>(&text).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

fn load_config_json(path: &Path) -> String {
    serde_json::to_string(&load_config_raw(path)).unwrap_or_else(|_| "{}".to_string())
}

/// Kana layout names available as `<name>.txt` in the XDG table directories.
fn kana_layouts() -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for dir in kana_table_search_dirs() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("txt") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        names.insert(stem.to_string());
                    }
                }
            }
        }
    }
    names.into_iter().collect()
}

fn kana_layouts_json() -> String {
    serde_json::to_string(&kana_layouts()).unwrap_or_else(|_| "[]".to_string())
}

/// Parses a JSON config and validates it (tilde-expanded, matching the daemon).
/// Returns `""` on success or a human-readable error message.
fn validate_config_json(json: &str) -> String {
    match serde_json::from_str::<Config>(json) {
        Ok(cfg) => match validate(&cfg.normalize()) {
            Ok(()) => String::new(),
            Err(e) => e.to_string(),
        },
        Err(e) => format!("internal: invalid config payload: {e}"),
    }
}

/// Validates the JSON config, writes it to `path` preserving comments/formatting,
/// then asks the daemon to reload.  Returns `""` on full success, otherwise a
/// message describing the failure stage.
fn apply_config_json(path: &Path, json: &str) -> String {
    let cfg: Config = match serde_json::from_str(json) {
        Ok(c) => c,
        Err(e) => return format!("internal: invalid config payload: {e}"),
    };
    if let Err(e) = validate(&cfg.clone().normalize()) {
        return e.to_string();
    }
    if let Err(e) = write_config_preserving(path, &cfg) {
        return format!("failed to write {}: {e}", path.display());
    }
    match skk_ipc::proxy::blocking::DaemonProxy::connect() {
        Ok(proxy) => match proxy.reload_config() {
            Ok(()) => String::new(),
            Err(e) => format!("Saved, but the daemon rejected the reload: {e}"),
        },
        Err(e) => format!("Saved, but could not reach the daemon (is it running?): {e}"),
    }
}

/// Writes `new` into the TOML document at `path`, updating only the keys that
/// differ from the on-disk values so comments, key order and untouched fields
/// (including `~` paths) are preserved.  A missing file is created from scratch.
fn write_config_preserving(path: &Path, new: &Config) -> std::io::Result<()> {
    let (mut doc, old) = match fs::read_to_string(path) {
        Ok(text) => (
            text.parse::<DocumentMut>().unwrap_or_default(),
            toml::from_str::<Config>(&text).unwrap_or_default(),
        ),
        Err(_) => (DocumentMut::new(), Config::default()),
    };

    // [input]
    let i = &new.input;
    let oi = &old.input;
    if i.kana_layout != oi.kana_layout {
        set_str(&mut doc, "input", "kana_layout", &i.kana_layout);
    }
    set_opt_path(
        &mut doc,
        "input",
        "kana_table",
        &i.kana_table,
        &oi.kana_table,
    );
    if i.default_mode != oi.default_mode {
        set_str(&mut doc, "input", "default_mode", &i.default_mode);
    }
    if i.toggle_keys != oi.toggle_keys {
        set_str_array(&mut doc, "input", "toggle_keys", &i.toggle_keys);
    }
    if i.raw_mode_toggle_keys != oi.raw_mode_toggle_keys {
        set_str_array(
            &mut doc,
            "input",
            "raw_mode_toggle_keys",
            &i.raw_mode_toggle_keys,
        );
    }
    if i.conversion_trigger_chars != oi.conversion_trigger_chars {
        set_str_array(
            &mut doc,
            "input",
            "conversion_trigger_chars",
            &i.conversion_trigger_chars,
        );
    }
    if i.vi_escape != oi.vi_escape {
        section_mut(&mut doc, "input").insert("vi_escape", value(i.vi_escape));
    }

    // [user-dict]
    set_opt_path(
        &mut doc,
        "user-dict",
        "path",
        &new.user_dict.path,
        &old.user_dict.path,
    );

    // [dict]
    if new.dict.lisp_directives != old.dict.lisp_directives {
        section_mut(&mut doc, "dict").insert("lisp_directives", value(new.dict.lisp_directives));
    }
    if new.dict.sources != old.dict.sources {
        let mut aot = ArrayOfTables::new();
        for s in &new.dict.sources {
            let mut t = Table::new();
            t.insert("path", value(s.path.to_string_lossy().as_ref()));
            t.insert("encoding", value(s.encoding.to_string()));
            t.insert("priority", value(i64::from(s.priority)));
            aot.push(t);
        }
        section_mut(&mut doc, "dict").insert("sources", Item::ArrayOfTables(aot));
    }

    // [indicator]
    if new.indicator.enabled != old.indicator.enabled {
        section_mut(&mut doc, "indicator").insert("enabled", value(new.indicator.enabled));
    }
    if new.indicator.timeout_ms != old.indicator.timeout_ms {
        section_mut(&mut doc, "indicator")
            .insert("timeout_ms", value(i64::from(new.indicator.timeout_ms)));
    }

    // [candidates]
    if new.candidates.inline_count != old.candidates.inline_count {
        section_mut(&mut doc, "candidates")
            .insert("inline_count", value(new.candidates.inline_count as i64));
    }
    if new.candidates.selection_keys != old.candidates.selection_keys {
        set_str(
            &mut doc,
            "candidates",
            "selection_keys",
            &new.candidates.selection_keys,
        );
    }

    // [daemon]
    if new.daemon.log_level != old.daemon.log_level {
        set_str(&mut doc, "daemon", "log_level", &new.daemon.log_level);
    }

    fs::write(path, doc.to_string())
}

// ── toml_edit helpers ───────────────────────────────────────────────────────

/// Returns a mutable reference to the named top-level table, creating it if it
/// does not exist (or is not a table).
fn section_mut<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    let root = doc.as_table_mut();
    if root.get(name).map_or(true, |it| !it.is_table()) {
        root.insert(name, Item::Table(Table::new()));
    }
    root.get_mut(name).unwrap().as_table_mut().unwrap()
}

fn set_str(doc: &mut DocumentMut, section: &str, key: &str, v: &str) {
    section_mut(doc, section).insert(key, value(v));
}

fn set_str_array(doc: &mut DocumentMut, section: &str, key: &str, vals: &[String]) {
    let mut arr = Array::new();
    for s in vals {
        arr.push(s.as_str());
    }
    section_mut(doc, section).insert(key, value(arr));
}

fn set_opt_path(
    doc: &mut DocumentMut,
    section: &str,
    key: &str,
    new: &Option<PathBuf>,
    old: &Option<PathBuf>,
) {
    if new == old {
        return;
    }
    match new {
        Some(p) => {
            section_mut(doc, section).insert(key, value(p.to_string_lossy().as_ref()));
        }
        None => {
            if let Some(t) = doc
                .as_table_mut()
                .get_mut(section)
                .and_then(Item::as_table_mut)
            {
                t.remove(key);
            }
        }
    }
}

// ── FFI boundary (C++ Qt front-end) ─────────────────────────────────────────

fn to_c(s: String) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

/// # Safety
/// `ptr` must be NULL or a valid NUL-terminated C string.
unsafe fn from_c(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Returns the current config (or defaults) as a JSON object string.
#[no_mangle]
pub extern "C" fn y2skk_settings_load_json() -> *mut c_char {
    to_c(load_config_json(&default_config_path()))
}

/// Returns the available kana layout names as a JSON array string.
#[no_mangle]
pub extern "C" fn y2skk_settings_kana_layouts_json() -> *mut c_char {
    to_c(kana_layouts_json())
}

/// Returns the config file path (for display) as a plain string.
#[no_mangle]
pub extern "C" fn y2skk_settings_default_config_path() -> *mut c_char {
    to_c(default_config_path().to_string_lossy().into_owned())
}

/// Validates a JSON config; returns `""` if valid or an error message.
///
/// # Safety
/// `json` must be NULL or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn y2skk_settings_validate_json(json: *const c_char) -> *mut c_char {
    to_c(validate_config_json(&from_c(json)))
}

/// Validates, writes (preserving comments) and reloads. Returns `""` on success.
///
/// # Safety
/// `json` must be NULL or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn y2skk_settings_apply_json(json: *const c_char) -> *mut c_char {
    to_c(apply_config_json(&default_config_path(), &from_c(json)))
}

/// Frees a string previously returned by this library.
///
/// # Safety
/// `ptr` must be NULL or a pointer returned by one of this library's functions.
#[no_mangle]
pub unsafe extern "C" fn y2skk_settings_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_back_preserves_comments_and_untouched_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "\
# y2skk configuration
[input]
# preferred layout
kana_layout = \"romaji\"
default_mode = \"ascii\"

[candidates]
inline_count = 3
selection_keys = \"asdfjkl;\"
";
        fs::write(&path, original).unwrap();

        let mut cfg = load_config_raw(&path);
        cfg.candidates.inline_count = 7; // change exactly one field
        write_config_preserving(&path, &cfg).unwrap();

        let out = fs::read_to_string(&path).unwrap();
        // Comments and untouched fields survive...
        assert!(out.contains("# y2skk configuration"));
        assert!(out.contains("# preferred layout"));
        assert!(out.contains("kana_layout = \"romaji\""));
        assert!(out.contains("default_mode = \"ascii\""));
        assert!(out.contains("selection_keys = \"asdfjkl;\""));
        // ...and the changed field is updated.
        assert!(out.contains("inline_count = 7"));
    }

    #[test]
    fn write_back_preserves_tilde_in_untouched_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[user-dict]\npath = \"~/.local/share/y2skk/user.dict\"\n",
        )
        .unwrap();

        let mut cfg = load_config_raw(&path);
        cfg.candidates.inline_count = 5; // unrelated change
        write_config_preserving(&path, &cfg).unwrap();

        let out = fs::read_to_string(&path).unwrap();
        assert!(out.contains("~/.local/share/y2skk/user.dict"));
    }

    #[test]
    fn validate_reports_bad_trigger_char() {
        let mut cfg = Config::default();
        cfg.input.conversion_trigger_chars = vec!["ab".to_string()]; // must be 1 char
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!validate_config_json(&json).is_empty());
    }

    #[test]
    fn load_missing_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let json = load_config_json(&dir.path().join("absent.toml"));
        let cfg: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg.input.kana_layout, Config::default().input.kana_layout);
    }
}
