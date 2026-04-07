/*
 * y2skk_im.h — interface between the GTK3 C shim and the Rust logic layer.
 *
 * The C shim owns all GObject/GType plumbing.  The Rust layer owns the D-Bus
 * session, key processing, and action dispatch.  They meet through this header.
 */

#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── Action kind constants (must match skk-ipc) ─────────────────────────── */

#define Y2SKK_ACTION_PASSTHROUGH     0
#define Y2SKK_ACTION_COMMIT          1
#define Y2SKK_ACTION_UPDATE_PREEDIT  2
#define Y2SKK_ACTION_CLEAR_PREEDIT   3
#define Y2SKK_ACTION_SHOW_CANDIDATES 4
#define Y2SKK_ACTION_HIDE_CANDIDATES 5

/* ── Callbacks provided by the C shim to Rust ───────────────────────────── */

typedef struct Y2skkCallbacks {
    /* Emit the "commit" signal with the given UTF-8 text. */
    void (*commit)(void *ctx, const char *text);

    /* Emit "preedit-changed" after updating internal preedit state.
     * text     — UTF-8 preedit string (empty when preedit is cleared)
     * cursor   — byte offset of cursor inside text */
    void (*update_preedit)(void *ctx, const char *text, uint32_t cursor);

    /* Clear preedit: shorthand for update_preedit(ctx, "", 0). */
    void (*clear_preedit)(void *ctx);

    /* Notify the shim that a candidate list is available.
     * words    — NULL-terminated array of UTF-8 candidate strings
     * focused  — index of the focused candidate */
    void (*show_candidates)(void *ctx, const char **words, uint32_t focused);

    /* Notify the shim that the candidate list should be hidden. */
    void (*hide_candidates)(void *ctx);
} Y2skkCallbacks;

/* ── Rust-side functions called from C ──────────────────────────────────── */

/*
 * Called once when the GTK module is loaded (gtk_im_module_init).
 * Establishes the D-Bus connection.  Returns 0 on success, non-zero on error.
 */
int y2skk_init(void);

/*
 * Called once when the GTK module is unloaded (gtk_im_module_exit).
 */
void y2skk_fini(void);

/*
 * Allocates a new IME session.  `app_id` is an arbitrary UTF-8 label.
 * Returns the session ID (> 0) on success, 0 on failure.
 */
uint32_t y2skk_create_session(const char *app_id);

/*
 * Releases a session previously created with y2skk_create_session.
 */
void y2skk_destroy_session(uint32_t session_id);

/*
 * Processes one key event.
 *
 * keyval     — GDK key value (maps to X11 keysym)
 * modifiers  — GDK modifier state bitmask
 * is_press   — non-zero for key-press, zero for key-release
 * ctx        — opaque pointer passed back verbatim to every callback
 * cbs        — function pointers for dispatching engine actions
 *
 * Returns non-zero if the key was consumed by the IME, zero for passthrough.
 */
int y2skk_process_key(
    uint32_t       session_id,
    uint32_t       keyval,
    uint32_t       modifiers,
    int            is_press,
    void          *ctx,
    const Y2skkCallbacks *cbs
);

/*
 * Notifies the IME that the context gained input focus.
 */
void y2skk_focus_in(uint32_t session_id);

/*
 * Notifies the IME that the context lost input focus.
 */
void y2skk_focus_out(uint32_t session_id);

/*
 * Resets the IME state for the given session (e.g. on Escape or focus change).
 */
void y2skk_reset(uint32_t session_id);

#ifdef __cplusplus
}
#endif
