#pragma once

// FFI surface implemented by the Rust `y2skk-settings` staticlib.
// All returned `char *` are UTF-8, owned by the caller, and must be released
// with y2skk_settings_string_free().

#ifdef __cplusplus
extern "C" {
#endif

// The crate version string (e.g. "0.9.5").
char *y2skk_settings_version(void);

// The full MIT license text.
char *y2skk_settings_license_text(void);

// Current config (or defaults) serialised as a JSON object string.
char *y2skk_settings_load_json(void);

// Available kana layout names as a JSON array of strings.
char *y2skk_settings_kana_layouts_json(void);

// The config file path (for display) as a plain string.
char *y2skk_settings_default_config_path(void);

// "" if the on-disk config is missing or valid, otherwise the TOML parse error
// (the form is still populated with defaults; the GUI should surface this).
char *y2skk_settings_load_error(void);

// Validates a JSON config; returns "" if valid, otherwise an error message.
char *y2skk_settings_validate_json(const char *json);

// Validates, writes config.toml (preserving comments) and reloads the daemon.
// Returns "" on success, otherwise an error message.
char *y2skk_settings_apply_json(const char *json);

// Frees a string returned by any of the functions above.
void y2skk_settings_string_free(char *ptr);

#ifdef __cplusplus
}
#endif
