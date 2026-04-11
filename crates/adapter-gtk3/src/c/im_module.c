/*
 * im_module.c — GTK3 IM module entry points and GObject subclass for y2skk.
 *
 * This file implements all GObject/GTK3 boilerplate.  Key-processing logic
 * lives on the Rust side (y2skk_im.h / lib.rs) and is invoked through the
 * Y2skkCallbacks dispatch table.
 */

#include <gtk/gtk.h>
#include <glib.h>
#include <string.h>

#include "y2skk_im.h"

/* ── Forward declarations ────────────────────────────────────────────────── */

static GType    y2skk_im_context_get_type (void);
static void     y2skk_im_context_class_init (GtkIMContextClass *klass);
static void     y2skk_im_context_init      (GtkIMContext      *ctx);
static void     y2skk_im_context_finalize  (GObject           *obj);

static gboolean y2skk_filter_keypress     (GtkIMContext *ctx, GdkEventKey *event);
static void     y2skk_focus_in_cb         (GtkIMContext *ctx);
static void     y2skk_focus_out_cb        (GtkIMContext *ctx);
static void     y2skk_reset_cb            (GtkIMContext *ctx);
static void     y2skk_get_preedit_string  (GtkIMContext *ctx,
                                           gchar       **str,
                                           PangoAttrList **attrs,
                                           gint         *cursor_pos);

/* ── Instance struct ─────────────────────────────────────────────────────── */

typedef struct _Y2skkIMContext      Y2skkIMContext;
typedef struct _Y2skkIMContextClass Y2skkIMContextClass;

struct _Y2skkIMContext {
    GtkIMContext   parent;
    uint32_t       session_id;   /* 0 = not yet allocated */
    gchar         *preedit_text; /* owned, may be NULL */
    gint           preedit_cursor; /* byte offset */
};

struct _Y2skkIMContextClass {
    GtkIMContextClass parent_class;
};

/* ── GType registration ──────────────────────────────────────────────────── */

static GType g_y2skk_type = 0;

static GType y2skk_im_context_get_type(void)
{
    if (g_y2skk_type == 0) {
        static const GTypeInfo info = {
            sizeof(Y2skkIMContextClass),
            NULL,                              /* base_init */
            NULL,                              /* base_finalize */
            (GClassInitFunc) y2skk_im_context_class_init,
            NULL,                              /* class_finalize */
            NULL,                              /* class_data */
            sizeof(Y2skkIMContext),
            0,
            (GInstanceInitFunc) y2skk_im_context_init,
            NULL
        };
        g_y2skk_type = g_type_register_static(
            GTK_TYPE_IM_CONTEXT,
            "Y2skkIMContext",
            &info,
            0
        );
    }
    return g_y2skk_type;
}

static void y2skk_im_context_class_init(GtkIMContextClass *klass)
{
    GObjectClass *obj_class = G_OBJECT_CLASS(klass);
    obj_class->finalize = y2skk_im_context_finalize;

    klass->filter_keypress    = y2skk_filter_keypress;
    klass->focus_in           = y2skk_focus_in_cb;
    klass->focus_out          = y2skk_focus_out_cb;
    klass->reset              = y2skk_reset_cb;
    klass->get_preedit_string = y2skk_get_preedit_string;
}

static void y2skk_im_context_init(GtkIMContext *ctx)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    self->session_id     = 0;
    self->preedit_text   = NULL;
    self->preedit_cursor = 0;

    self->session_id = y2skk_create_session("gtk3");
}

static void y2skk_im_context_finalize(GObject *obj)
{
    Y2skkIMContext *self = (Y2skkIMContext *)obj;

    if (self->session_id != 0) {
        y2skk_destroy_session(self->session_id);
        self->session_id = 0;
    }
    g_free(self->preedit_text);
    self->preedit_text = NULL;

    G_OBJECT_CLASS(g_type_class_peek_parent(G_OBJECT_GET_CLASS(obj)))->finalize(obj);
}

/* ── Action callbacks (C → GTK signals / state updates) ─────────────────── */

static void cb_commit(void *ctx_ptr, const char *text)
{
    GtkIMContext *ctx = (GtkIMContext *)ctx_ptr;
    g_signal_emit_by_name(ctx, "commit", text);
}

static void cb_update_preedit(void *ctx_ptr, const char *text, uint32_t cursor)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx_ptr;

    g_free(self->preedit_text);
    self->preedit_text   = g_strdup(text ? text : "");
    self->preedit_cursor = (gint)cursor;

    g_signal_emit_by_name(ctx_ptr, "preedit-changed");
}

static void cb_clear_preedit(void *ctx_ptr)
{
    cb_update_preedit(ctx_ptr, "", 0);
}

static void cb_show_candidates(void *ctx_ptr, const char **words, uint32_t focused)
{
    /* Candidate window is not yet implemented; log and ignore. */
    (void)ctx_ptr; (void)words; (void)focused;
}

static void cb_hide_candidates(void *ctx_ptr)
{
    (void)ctx_ptr;
}

static const Y2skkCallbacks g_callbacks = {
    .commit          = cb_commit,
    .update_preedit  = cb_update_preedit,
    .clear_preedit   = cb_clear_preedit,
    .show_candidates = cb_show_candidates,
    .hide_candidates = cb_hide_candidates,
};

/* ── GtkIMContext virtual function implementations ───────────────────────── */

static gboolean y2skk_filter_keypress(GtkIMContext *ctx, GdkEventKey *event)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (self->session_id == 0)
        return FALSE;

    int consumed = y2skk_process_key(
        self->session_id,
        event->keyval,
        event->state,
        (event->type == GDK_KEY_PRESS) ? 1 : 0,
        ctx,
        &g_callbacks
    );
    return consumed ? TRUE : FALSE;
}

static void y2skk_focus_in_cb(GtkIMContext *ctx)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (self->session_id != 0)
        y2skk_focus_in(self->session_id);
}

static void y2skk_focus_out_cb(GtkIMContext *ctx)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (self->session_id != 0)
        y2skk_focus_out(self->session_id);
}

static void y2skk_reset_cb(GtkIMContext *ctx)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (self->session_id != 0)
        y2skk_reset(self->session_id);
}

static void y2skk_get_preedit_string(GtkIMContext  *ctx,
                                     gchar        **str,
                                     PangoAttrList **attrs,
                                     gint          *cursor_pos)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;

    if (str)
        *str = g_strdup(self->preedit_text ? self->preedit_text : "");

    if (cursor_pos)
        *cursor_pos = self->preedit_cursor;

    if (attrs) {
        *attrs = pango_attr_list_new();
        const gchar *text = self->preedit_text ? self->preedit_text : "";
        guint len = (guint)strlen(text);
        if (len > 0) {
            /* Underline the whole preedit string */
            PangoAttribute *uline = pango_attr_underline_new(PANGO_UNDERLINE_SINGLE);
            uline->start_index = 0;
            uline->end_index   = len;
            pango_attr_list_insert(*attrs, uline);
        }
    }
}

/* ── GTK3 IM module entry points ─────────────────────────────────────────── */

static const GtkIMContextInfo y2skk_info = {
    .context_id      = "y2skk",
    .context_name    = "y2skk Japanese Input",
    .domain          = "y2skk",
    .domain_dirname  = "",
    .default_locales = "ja:*",
};

static const GtkIMContextInfo *y2skk_info_list[] = { &y2skk_info };

/* These functions are called from Rust #[no_mangle] wrappers (lib.rs) so that
 * the GTK3 IM module entry points end up in the cdylib's dynamic symbol table.
 * Rust's cdylib linker version script only exports #[no_mangle] pub extern "C"
 * symbols, so the Rust side owns the public names; the C side provides the
 * implementations under internal _y2skk_ prefixed names. */

void
_y2skk_im_module_list(const GtkIMContextInfo ***contexts, int *n_contexts)
{
    *contexts   = y2skk_info_list;
    *n_contexts = G_N_ELEMENTS(y2skk_info_list);
}

void
_y2skk_im_module_init(GTypeModule *module)
{
    (void)module;
    y2skk_im_context_get_type(); /* registers the GType */
    if (y2skk_init() != 0)
        g_warning("y2skk: failed to connect to D-Bus daemon");
}

void
_y2skk_im_module_exit(void)
{
    y2skk_fini();
}

GtkIMContext *
_y2skk_im_module_create(const gchar *context_id)
{
    if (g_strcmp0(context_id, "y2skk") == 0)
        return GTK_IM_CONTEXT(g_object_new(y2skk_im_context_get_type(), NULL));
    return NULL;
}
