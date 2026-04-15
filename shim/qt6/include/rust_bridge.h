/*
 * rust_bridge.h — C interface between the Qt6 C++ shim and the Rust logic layer.
 *
 * The C++ side owns all Qt/QObject plumbing.  The Rust side owns the D-Bus
 * session and key-processing logic.  They communicate through this header.
 *
 * Naming mirrors the GTK3 adapter (y2skk_im.h) intentionally so both adapters
 * present the same conceptual interface to the Rust engine layer.
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
#define Y2SKK_ACTION_UPDATE_STATUS   6

/* ── Action callbacks provided by C++ to Rust ───────────────────────────── */

typedef struct Y2skkCallbacks {
    /* Commit text to the focused widget. */
    void (*commit)(void *ctx, const char *text);

    /* Update the preedit string.
     * text        — UTF-8 preedit (may be empty)
     * cursor      — byte offset of cursor inside text
     * ghost_start — byte offset where completion ghost begins; UINT32_MAX = no ghost */
    void (*update_preedit)(void *ctx, const char *text, uint32_t cursor, uint32_t ghost_start);

    /* Clear the preedit string (shorthand for update_preedit with empty text). */
    void (*clear_preedit)(void *ctx);

    /* Show a candidate list (listing mode only; never called during inline candidates).
     * words   — NULL-terminated array of UTF-8 candidate strings for the current page
     * focused — index within the page of the focused candidate (currently always 0)
     * keys    — selection key characters as a UTF-8 string (e.g. "asdfjkl;"); words[i]
     *           is selected by pressing keys[i] */
    void (*show_candidates)(void *ctx, const char **words, uint32_t focused, const char *keys);

    /* Hide the candidate list. */
    void (*hide_candidates)(void *ctx);

    /* Show the mode indicator popup (e.g. "あ", "ア", "a").
     * indicator — UTF-8 string of the mode indicator character */
    void (*update_status)(void *ctx, const char *indicator);
} Y2skkCallbacks;

/* ── Rust-side functions called from C++ ────────────────────────────────── */

/* Called once at plugin load time.  Returns 0 on success. */
int  y2skk_init(void);

/* Called once when the plugin is unloaded. */
void y2skk_fini(void);

/* Allocates a new IME session.  Returns the session ID (> 0), or 0 on error. */
uint32_t y2skk_create_session(const char *app_id);

/* Releases a session. */
void y2skk_destroy_session(uint32_t session_id);

/*
 * Processes one key event.
 *
 * key_sym   — Qt key value (Qt::Key enum integer)
 * modifiers — Qt::KeyboardModifiers bitmask
 * is_press  — non-zero for key-press, zero for key-release
 * ctx       — opaque pointer passed verbatim to every callback
 * cbs       — action dispatch table
 *
 * Returns non-zero if the IME consumed the key, zero for passthrough.
 */
int y2skk_process_key(
    uint32_t              session_id,
    uint32_t              key_sym,
    uint32_t              modifiers,
    int                   is_press,
    void                 *ctx,
    const Y2skkCallbacks *cbs
);

/* Notify focus gained. */
void y2skk_focus_in(uint32_t session_id);

/* Notify focus lost. */
void y2skk_focus_out(uint32_t session_id);

/* Reset IME state (e.g. on Escape or input-method disable). */
void y2skk_reset(uint32_t session_id);

#ifdef __cplusplus
} /* extern "C" */
#endif
