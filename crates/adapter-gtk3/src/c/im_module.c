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

static gboolean y2skk_filter_keypress      (GtkIMContext *ctx, GdkEventKey *event);
static void     y2skk_focus_in_cb          (GtkIMContext *ctx);
static void     y2skk_focus_out_cb         (GtkIMContext *ctx);
static void     y2skk_reset_cb             (GtkIMContext *ctx);
static void     y2skk_get_preedit_string   (GtkIMContext *ctx,
                                            gchar       **str,
                                            PangoAttrList **attrs,
                                            gint         *cursor_pos);
static void     y2skk_set_client_window    (GtkIMContext *ctx, GdkWindow *window);
static void     y2skk_set_cursor_location  (GtkIMContext *ctx, GdkRectangle *area);

/* ── Instance struct ─────────────────────────────────────────────────────── */

typedef struct _Y2skkIMContext      Y2skkIMContext;
typedef struct _Y2skkIMContextClass Y2skkIMContextClass;

struct _Y2skkIMContext {
    GtkIMContext   parent;
    uint32_t       session_id;     /* 0 = not yet allocated */
    gchar         *preedit_text;   /* owned, may be NULL */
    gint           preedit_cursor; /* byte offset */
    GdkWindow     *client_window;  /* window the context is attached to */
    GdkRectangle   cursor_area;    /* last known cursor rectangle (widget coords) */
    GtkWidget     *cand_window;    /* floating candidate popup, or NULL */
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

    klass->filter_keypress     = y2skk_filter_keypress;
    klass->focus_in            = y2skk_focus_in_cb;
    klass->focus_out           = y2skk_focus_out_cb;
    klass->reset               = y2skk_reset_cb;
    klass->get_preedit_string  = y2skk_get_preedit_string;
    klass->set_client_window   = y2skk_set_client_window;
    klass->set_cursor_location = y2skk_set_cursor_location;
}

static void y2skk_im_context_init(GtkIMContext *ctx)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    self->session_id     = 0;
    self->preedit_text   = NULL;
    self->preedit_cursor = 0;
    self->client_window  = NULL;
    memset(&self->cursor_area, 0, sizeof(self->cursor_area));
    self->cand_window    = NULL;

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

    if (self->cand_window) {
        gtk_widget_destroy(self->cand_window);
        self->cand_window = NULL;
    }
    if (self->client_window) {
        g_object_unref(self->client_window);
        self->client_window = NULL;
    }

    G_OBJECT_CLASS(g_type_class_peek_parent(G_OBJECT_GET_CLASS(obj)))->finalize(obj);
}

/* ── Candidate window helpers ────────────────────────────────────────────── */

/* Apply CSS to style the candidate popup on first call. */
static void ensure_candidate_css(void)
{
    static gboolean installed = FALSE;
    if (installed)
        return;
    installed = TRUE;

    GtkCssProvider *css = gtk_css_provider_new();
    gtk_css_provider_load_from_data(css,
        ".y2skk-cand-window {"
        "  background-color: @theme_base_color;"
        "  border: 1px solid @theme_fg_color;"
        "}"
        ".y2skk-cand-row {"
        "  padding: 2px 6px;"
        "}"
        ".y2skk-cand-focused {"
        "  background-color: @theme_selected_bg_color;"
        "  color: @theme_selected_fg_color;"
        "}",
        -1, NULL);
    gtk_style_context_add_provider_for_screen(
        gdk_screen_get_default(),
        GTK_STYLE_PROVIDER(css),
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
    g_object_unref(css);
}

static void cand_window_destroy(Y2skkIMContext *self)
{
    if (self->cand_window) {
        gtk_widget_destroy(self->cand_window);
        self->cand_window = NULL;
    }
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

static void cb_show_candidates(void *ctx_ptr, const char **words, uint32_t focused,
                                const char *sel_keys)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx_ptr;

    /* Count candidates. */
    int n = 0;
    while (words[n]) n++;
    if (n == 0) {
        cand_window_destroy(self);
        return;
    }

    ensure_candidate_css();

    /* Reuse existing window if present, otherwise create one. */
    if (!self->cand_window) {
        GtkWidget *win = gtk_window_new(GTK_WINDOW_POPUP);
        gtk_window_set_resizable(GTK_WINDOW(win), FALSE);
        gtk_window_set_skip_taskbar_hint(GTK_WINDOW(win), TRUE);
        gtk_window_set_skip_pager_hint(GTK_WINDOW(win), TRUE);

        GtkStyleContext *wsc = gtk_widget_get_style_context(win);
        gtk_style_context_add_class(wsc, "y2skk-cand-window");

        GtkWidget *frame = gtk_frame_new(NULL);
        gtk_frame_set_shadow_type(GTK_FRAME(frame), GTK_SHADOW_OUT);
        gtk_container_add(GTK_CONTAINER(win), frame);

        GtkWidget *vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_container_add(GTK_CONTAINER(frame), vbox);

        /* Store vbox as window data for rebuilding rows. */
        g_object_set_data(G_OBJECT(win), "vbox", vbox);

        self->cand_window = win;
    }

    /* Rebuild candidate rows. */
    GtkWidget *vbox = GTK_WIDGET(g_object_get_data(G_OBJECT(self->cand_window), "vbox"));
    GList *children = gtk_container_get_children(GTK_CONTAINER(vbox));
    g_list_foreach(children, (GFunc)gtk_widget_destroy, NULL);
    g_list_free(children);

    int n_keys = sel_keys ? (int)strlen(sel_keys) : 0;
    for (int i = 0; i < n; i++) {
        GtkWidget *row = gtk_event_box_new();
        GtkStyleContext *rsc = gtk_widget_get_style_context(row);
        gtk_style_context_add_class(rsc, "y2skk-cand-row");
        if (i == (int)focused)
            gtk_style_context_add_class(rsc, "y2skk-cand-focused");

        gchar *text;
        if (i < n_keys)
            text = g_strdup_printf("%c: %s", sel_keys[i], words[i]);
        else
            text = g_strdup(words[i]);

        GtkWidget *label = gtk_label_new(text);
        gtk_label_set_xalign(GTK_LABEL(label), 0.0);
        g_free(text);

        gtk_container_add(GTK_CONTAINER(row), label);
        gtk_box_pack_start(GTK_BOX(vbox), row, FALSE, FALSE, 0);
    }

    gtk_widget_show_all(self->cand_window);

    /* Position below the cursor. */
    gint ox = 0, oy = 0;
    if (self->client_window)
        gdk_window_get_origin(self->client_window, &ox, &oy);

    gint x = ox + self->cursor_area.x;
    gint y = oy + self->cursor_area.y + self->cursor_area.height;

    /* Clamp to the monitor that contains the cursor position. */
    GdkDisplay  *display = gdk_display_get_default();
    GdkMonitor  *monitor = gdk_display_get_monitor_at_point(display, x, y);
    GdkRectangle mgeom;
    gdk_monitor_get_geometry(monitor, &mgeom);

    GtkRequisition req;
    gtk_widget_get_preferred_size(self->cand_window, NULL, &req);

    if (x + req.width  > mgeom.x + mgeom.width)
        x = mgeom.x + mgeom.width  - req.width;
    if (y + req.height > mgeom.y + mgeom.height)
        y = oy + self->cursor_area.y - req.height; /* flip above cursor */
    if (x < mgeom.x) x = mgeom.x;
    if (y < mgeom.y) y = mgeom.y;

    gtk_window_move(GTK_WINDOW(self->cand_window), x, y);
}

static void cb_hide_candidates(void *ctx_ptr)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx_ptr;
    cand_window_destroy(self);
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

static void y2skk_set_client_window(GtkIMContext *ctx, GdkWindow *window)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (self->client_window)
        g_object_unref(self->client_window);
    self->client_window = window ? g_object_ref(window) : NULL;
}

static void y2skk_set_cursor_location(GtkIMContext *ctx, GdkRectangle *area)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (area)
        self->cursor_area = *area;

    /* Reposition the candidate window if it is currently visible. */
    if (self->cand_window && gtk_widget_get_visible(self->cand_window)) {
        gint ox = 0, oy = 0;
        if (self->client_window)
            gdk_window_get_origin(self->client_window, &ox, &oy);

        gint x = ox + self->cursor_area.x;
        gint y = oy + self->cursor_area.y + self->cursor_area.height;

        GdkDisplay  *display = gdk_display_get_default();
        GdkMonitor  *monitor = gdk_display_get_monitor_at_point(display, x, y);
        GdkRectangle mgeom;
        gdk_monitor_get_geometry(monitor, &mgeom);

        GtkRequisition req;
        gtk_widget_get_preferred_size(self->cand_window, NULL, &req);

        if (x + req.width  > mgeom.x + mgeom.width)  x = mgeom.x + mgeom.width  - req.width;
        if (y + req.height > mgeom.y + mgeom.height)  y = oy + self->cursor_area.y - req.height;
        if (x < mgeom.x) x = mgeom.x;
        if (y < mgeom.y) y = mgeom.y;

        gtk_window_move(GTK_WINDOW(self->cand_window), x, y);
    }
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
    cand_window_destroy(self);
}

static void y2skk_reset_cb(GtkIMContext *ctx)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (self->session_id != 0)
        y2skk_reset(self->session_id);
    cand_window_destroy(self);
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
