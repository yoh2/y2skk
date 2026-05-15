/*
 * im_module.c — GTK4 IM module entry points and GObject subclass for y2skk.
 *
 * Mirrors the structure of crates/adapter-gtk3/src/c/im_module.c but uses the
 * GTK4 input-method API (GIO extension point, GtkPopover-based popups,
 * GdkEvent accessors, etc.).  Key-processing logic lives on the Rust side
 * (y2skk_im.h / lib.rs) and is invoked through the Y2skkCallbacks dispatch
 * table.
 */

#include <gio/gio.h>
#include <gtk/gtk.h>
#include <glib.h>
#include <string.h>

#include "y2skk_im.h"

/* GTK4 advertises this constant for the IM module extension point. */
#ifndef GTK_IM_MODULE_EXTENSION_POINT_NAME
#define GTK_IM_MODULE_EXTENSION_POINT_NAME "gtk-im-module"
#endif

/* ── Instance struct ─────────────────────────────────────────────────────── */

typedef struct _Y2skkIMContext      Y2skkIMContext;
typedef struct _Y2skkIMContextClass Y2skkIMContextClass;

struct _Y2skkIMContext {
    GtkIMContext   parent;
    uint32_t       session_id;          /* 0 = not yet allocated */
    gchar         *preedit_text;        /* owned, may be NULL */
    gint           preedit_cursor;      /* byte offset */
    uint32_t       preedit_ghost_start; /* byte offset where ghost begins; UINT32_MAX = no ghost */
    GtkWidget     *client_widget;       /* widget the context is attached to (weak ref) */
    GdkRectangle   cursor_area;         /* last known cursor rectangle (widget coords) */
    GtkWidget     *cand_popover;        /* GtkPopover for candidates, or NULL */
    GtkWidget     *cand_box;            /* vbox inside the candidate popover */
    GtkWidget     *status_popover;      /* GtkPopover for mode indicator, or NULL */
    GtkWidget     *status_label;        /* label inside the status popover */
    guint          status_timer_id;     /* GSource ID for auto-hide (0 = none) */
};

struct _Y2skkIMContextClass {
    GtkIMContextClass parent_class;
};

/* G_DEFINE_DYNAMIC_TYPE generates y2skk_im_context_get_type(),
 * y2skk_im_context_register_type(), and the static
 * `y2skk_im_context_parent_class` variable. */
static void y2skk_im_context_class_init     (Y2skkIMContextClass *klass);
static void y2skk_im_context_class_finalize (Y2skkIMContextClass *klass);
static void y2skk_im_context_init           (Y2skkIMContext      *self);
static void y2skk_im_context_finalize       (GObject             *obj);

G_DEFINE_DYNAMIC_TYPE(Y2skkIMContext, y2skk_im_context, GTK_TYPE_IM_CONTEXT)

/* Forward declarations for IMContext virtual methods. */
static gboolean y2skk_filter_keypress     (GtkIMContext *ctx, GdkEvent *event);
static void     y2skk_focus_in_cb         (GtkIMContext *ctx);
static void     y2skk_focus_out_cb        (GtkIMContext *ctx);
static void     y2skk_reset_cb            (GtkIMContext *ctx);
static void     y2skk_get_preedit_string  (GtkIMContext  *ctx,
                                           gchar        **str,
                                           PangoAttrList **attrs,
                                           gint          *cursor_pos);
static void     y2skk_set_client_widget   (GtkIMContext *ctx, GtkWidget *widget);
static void     y2skk_set_cursor_location (GtkIMContext *ctx, GdkRectangle *area);

/* Forward declarations for popover helpers. */
static void cand_popover_destroy (Y2skkIMContext *self);
static void status_popover_destroy (Y2skkIMContext *self);
static void reposition_popover (Y2skkIMContext *self, GtkWidget *popover);

/* ── Class boilerplate ───────────────────────────────────────────────────── */

static void y2skk_im_context_class_init(Y2skkIMContextClass *klass)
{
    GObjectClass      *obj_class = G_OBJECT_CLASS(klass);
    GtkIMContextClass *im_class  = GTK_IM_CONTEXT_CLASS(klass);

    obj_class->finalize           = y2skk_im_context_finalize;
    im_class->filter_keypress     = y2skk_filter_keypress;
    im_class->focus_in            = y2skk_focus_in_cb;
    im_class->focus_out           = y2skk_focus_out_cb;
    im_class->reset               = y2skk_reset_cb;
    im_class->get_preedit_string  = y2skk_get_preedit_string;
    im_class->set_client_widget   = y2skk_set_client_widget;
    im_class->set_cursor_location = y2skk_set_cursor_location;
}

/* G_DEFINE_DYNAMIC_TYPE requires class_finalize even if it is empty. */
static void y2skk_im_context_class_finalize(Y2skkIMContextClass *klass)
{
    (void)klass;
}

static void y2skk_im_context_init(Y2skkIMContext *self)
{
    self->session_id          = 0;
    self->preedit_text        = NULL;
    self->preedit_cursor      = 0;
    self->preedit_ghost_start = UINT32_MAX;
    self->client_widget       = NULL;
    memset(&self->cursor_area, 0, sizeof(self->cursor_area));
    self->cand_popover        = NULL;
    self->cand_box            = NULL;
    self->status_popover      = NULL;
    self->status_label        = NULL;
    self->status_timer_id     = 0;

    self->session_id = y2skk_create_session("gtk4");
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

    cand_popover_destroy(self);
    if (self->status_timer_id != 0) {
        g_source_remove(self->status_timer_id);
        self->status_timer_id = 0;
    }
    status_popover_destroy(self);
    /* client_widget is held by weak reference (gtk_widget_set_parent on the
     * popover handles widget hierarchy). Nothing to release here. */

    G_OBJECT_CLASS(y2skk_im_context_parent_class)->finalize(obj);
}

/* ── CSS for popover styling ─────────────────────────────────────────────── */

static void ensure_popover_css(Y2skkIMContext *self)
{
    static gboolean installed = FALSE;
    if (installed)
        return;
    if (!self->client_widget)
        return;

    GdkDisplay *display = gtk_widget_get_display(self->client_widget);
    if (!display)
        return;

    GtkCssProvider *css = gtk_css_provider_new();
    /* gtk_css_provider_load_from_data is deprecated in GTK 4.12 in favour of
     * gtk_css_provider_load_from_string, but we want to keep the minimum
     * supported version at 4.0 (build.rs probe). Silence the deprecation
     * warning for this one call. */
    G_GNUC_BEGIN_IGNORE_DEPRECATIONS
    gtk_css_provider_load_from_data(css,
        ".y2skk-cand-row {"
        "  padding: 2px 6px;"
        "}"
        ".y2skk-cand-focused {"
        "  background-color: @theme_selected_bg_color;"
        "  color: @theme_selected_fg_color;"
        "}"
        ".y2skk-status-label {"
        "  padding: 2px 6px;"
        "  font-size: larger;"
        "}",
        -1);
    G_GNUC_END_IGNORE_DEPRECATIONS
    gtk_style_context_add_provider_for_display(
        display,
        GTK_STYLE_PROVIDER(css),
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
    g_object_unref(css);
    installed = TRUE;
}

/* ── Popover helpers ─────────────────────────────────────────────────────── */

static void cand_popover_destroy(Y2skkIMContext *self)
{
    if (self->cand_popover) {
        gtk_widget_unparent(self->cand_popover);
        self->cand_popover = NULL;
        self->cand_box     = NULL;
    }
}

static void status_popover_destroy(Y2skkIMContext *self)
{
    if (self->status_popover) {
        gtk_widget_unparent(self->status_popover);
        self->status_popover = NULL;
        self->status_label   = NULL;
    }
}

/* Position a GtkPopover relative to the cached cursor rectangle on the
 * client widget. Both popovers are children of the client widget so
 * gtk_popover_set_pointing_to() takes coordinates in widget space. */
static void reposition_popover(Y2skkIMContext *self, GtkWidget *popover)
{
    if (!popover || !self->client_widget)
        return;
    gtk_popover_set_pointing_to(GTK_POPOVER(popover), &self->cursor_area);
}

/* ── Action callbacks (C → GTK signals / state updates) ─────────────────── */

static void cb_commit(void *ctx_ptr, const char *text)
{
    GtkIMContext *ctx = (GtkIMContext *)ctx_ptr;
    g_signal_emit_by_name(ctx, "commit", text);
}

static void cb_update_preedit(void *ctx_ptr, const char *text, uint32_t cursor,
                               uint32_t ghost_start)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx_ptr;

    g_free(self->preedit_text);
    self->preedit_text        = g_strdup(text ? text : "");
    self->preedit_cursor      = (gint)cursor;
    self->preedit_ghost_start = ghost_start;

    g_signal_emit_by_name(ctx_ptr, "preedit-changed");
}

static void cb_clear_preedit(void *ctx_ptr)
{
    cb_update_preedit(ctx_ptr, "", 0, UINT32_MAX);
}

static void cb_show_candidates(void *ctx_ptr, const char **words, uint32_t focused,
                                const char *sel_keys)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx_ptr;

    /* Count candidates. */
    int n = 0;
    while (words[n]) n++;
    if (n == 0) {
        cand_popover_destroy(self);
        return;
    }

    /* No client widget yet — popovers need a parent to attach to. */
    if (!self->client_widget)
        return;

    ensure_popover_css(self);

    /* Re-create the popover from scratch each time the candidate set changes.
     * GtkPopover's child swap dance is finicky and the candidate list usually
     * differs (focused candidate at minimum), so destroy + rebuild is simpler. */
    cand_popover_destroy(self);

    GtkWidget *popover = gtk_popover_new();
    gtk_popover_set_autohide(GTK_POPOVER(popover), FALSE);
    gtk_popover_set_has_arrow(GTK_POPOVER(popover), FALSE);
    gtk_popover_set_position(GTK_POPOVER(popover), GTK_POS_BOTTOM);
    gtk_widget_set_parent(popover, self->client_widget);

    GtkWidget *vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_popover_set_child(GTK_POPOVER(popover), vbox);

    int n_keys = sel_keys ? (int)strlen(sel_keys) : 0;
    for (int i = 0; i < n; i++) {
        gchar *text;
        if (i < n_keys)
            text = g_strdup_printf("%c: %s", sel_keys[i], words[i]);
        else
            text = g_strdup(words[i]);

        GtkWidget *label = gtk_label_new(text);
        gtk_label_set_xalign(GTK_LABEL(label), 0.0);
        g_free(text);

        gtk_widget_add_css_class(label, "y2skk-cand-row");
        if (i == (int)focused)
            gtk_widget_add_css_class(label, "y2skk-cand-focused");

        gtk_box_append(GTK_BOX(vbox), label);
    }

    self->cand_popover = popover;
    self->cand_box     = vbox;

    reposition_popover(self, popover);
    gtk_popover_popup(GTK_POPOVER(popover));
}

static void cb_hide_candidates(void *ctx_ptr)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx_ptr;
    cand_popover_destroy(self);
}

/* Auto-hide timer callback for the status popover. */
static gboolean status_popover_hide_cb(gpointer user_data)
{
    Y2skkIMContext *self = (Y2skkIMContext *)user_data;
    if (self->status_popover)
        gtk_popover_popdown(GTK_POPOVER(self->status_popover));
    self->status_timer_id = 0;
    return G_SOURCE_REMOVE;
}

static void cb_update_status(void *ctx_ptr, const char *label, uint32_t timeout_ms)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx_ptr;

    /* No client widget yet — popovers need a parent. */
    if (!self->client_widget)
        return;

    ensure_popover_css(self);

    /* Cancel any running auto-hide timer. */
    if (self->status_timer_id != 0) {
        g_source_remove(self->status_timer_id);
        self->status_timer_id = 0;
    }

    /* Create the status popover on first use. */
    if (!self->status_popover) {
        GtkWidget *popover = gtk_popover_new();
        gtk_popover_set_autohide(GTK_POPOVER(popover), FALSE);
        gtk_popover_set_has_arrow(GTK_POPOVER(popover), FALSE);
        gtk_popover_set_position(GTK_POPOVER(popover), GTK_POS_BOTTOM);
        gtk_widget_set_parent(popover, self->client_widget);

        GtkWidget *lbl = gtk_label_new(NULL);
        gtk_widget_add_css_class(lbl, "y2skk-status-label");
        gtk_popover_set_child(GTK_POPOVER(popover), lbl);

        self->status_popover = popover;
        self->status_label   = lbl;
    }

    gtk_label_set_text(GTK_LABEL(self->status_label), label ? label : "");
    reposition_popover(self, self->status_popover);
    gtk_popover_popup(GTK_POPOVER(self->status_popover));

    /* Schedule auto-hide if a timeout was requested. */
    if (timeout_ms > 0)
        self->status_timer_id = g_timeout_add(timeout_ms, status_popover_hide_cb, self);
}

static const Y2skkCallbacks g_callbacks = {
    .commit          = cb_commit,
    .update_preedit  = cb_update_preedit,
    .clear_preedit   = cb_clear_preedit,
    .show_candidates = cb_show_candidates,
    .hide_candidates = cb_hide_candidates,
    .update_status   = cb_update_status,
};

/* ── GtkIMContext virtual function implementations ───────────────────────── */

static gboolean y2skk_filter_keypress(GtkIMContext *ctx, GdkEvent *event)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (self->session_id == 0 || event == NULL)
        return FALSE;

    GdkEventType type = gdk_event_get_event_type(event);
    if (type != GDK_KEY_PRESS && type != GDK_KEY_RELEASE)
        return FALSE;

    guint           keyval     = gdk_key_event_get_keyval(event);
    GdkModifierType modifiers  = gdk_event_get_modifier_state(event);

    int consumed = y2skk_process_key(
        self->session_id,
        keyval,
        (uint32_t)modifiers,
        (type == GDK_KEY_PRESS) ? 1 : 0,
        ctx,
        &g_callbacks
    );
    return consumed ? TRUE : FALSE;
}

static void y2skk_set_client_widget(GtkIMContext *ctx, GtkWidget *widget)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;

    /* Tear down popovers whose parent is about to become invalid. */
    cand_popover_destroy(self);
    status_popover_destroy(self);

    /* GtkIMContext does not retain the widget; we keep a borrowed pointer.
     * The widget is guaranteed to outlive its IMContext (or set_client_widget
     * will be called again with a different value before it goes away). */
    self->client_widget = widget;
}

static void y2skk_set_cursor_location(GtkIMContext *ctx, GdkRectangle *area)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (area)
        self->cursor_area = *area;

    if (self->cand_popover && gtk_widget_get_visible(self->cand_popover))
        reposition_popover(self, self->cand_popover);

    if (self->status_popover && gtk_widget_get_visible(self->status_popover))
        reposition_popover(self, self->status_popover);
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
    cand_popover_destroy(self);
}

static void y2skk_reset_cb(GtkIMContext *ctx)
{
    Y2skkIMContext *self = (Y2skkIMContext *)ctx;
    if (self->session_id != 0)
        y2skk_reset(self->session_id);
    cand_popover_destroy(self);
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
            uint32_t gs = self->preedit_ghost_start;
            /* Clamp ghost_start to a valid byte boundary within text. */
            if (gs > len) gs = len;

            if (gs > 0) {
                /* Underline only the user-typed portion (0..ghost_start). */
                PangoAttribute *uline = pango_attr_underline_new(PANGO_UNDERLINE_SINGLE);
                uline->start_index = 0;
                uline->end_index   = gs;
                pango_attr_list_insert(*attrs, uline);
            } else {
                /* No ghost: underline everything. */
                PangoAttribute *uline = pango_attr_underline_new(PANGO_UNDERLINE_SINGLE);
                uline->start_index = 0;
                uline->end_index   = len;
                pango_attr_list_insert(*attrs, uline);
            }

            if (gs < len) {
                /* Ghost portion: grey foreground, no underline. */
                PangoAttribute *fg = pango_attr_foreground_new(0x8080, 0x8080, 0x8080);
                fg->start_index = gs;
                fg->end_index   = len;
                pango_attr_list_insert(*attrs, fg);
            }
        }
    }
}

/* ── GIO module entry points (called by GTK4 via dlsym) ──────────────────── */

/* These are wrapped by Rust #[no_mangle] thunks (lib.rs) so the cdylib
 * actually exports `g_io_module_load`, `g_io_module_unload`, and
 * `g_io_module_query` — Rust's cdylib linker version script only exports
 * symbols declared on the Rust side. */

void
_y2skk_g_io_module_load(GIOModule *module)
{
    /* Register the dynamic GtkIMContext subclass against the supplied
     * type module.  G_DEFINE_DYNAMIC_TYPE generated this _register_type
     * function for us. */
    y2skk_im_context_register_type(G_TYPE_MODULE(module));

    /* Implement the gtk-im-module extension point so GTK4 can find us
     * when the user sets GTK_IM_MODULE=y2skk. */
    g_io_extension_point_implement(
        GTK_IM_MODULE_EXTENSION_POINT_NAME,
        y2skk_im_context_get_type(),
        "y2skk",
        /* priority — higher means more preferred among modules */ 50
    );

    if (y2skk_init() != 0)
        g_warning("y2skk: failed to connect to D-Bus daemon");
}

void
_y2skk_g_io_module_unload(GIOModule *module)
{
    (void)module;
    y2skk_fini();
}

char **
_y2skk_g_io_module_query(void)
{
    static const gchar *eps[] = { GTK_IM_MODULE_EXTENSION_POINT_NAME, NULL };
    return g_strdupv((gchar **)eps);
}
