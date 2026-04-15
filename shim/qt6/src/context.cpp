/*
 * context.cpp — Y2skkInputContext implementation.
 *
 * All actual IME logic lives in the Rust staticlib (adapter-qt6).
 * This file handles only the Qt object plumbing and the translation of
 * Qt events / signals into calls to the Rust layer via rust_bridge.h.
 */

#include "context.h"

#include <QtGui/QGuiApplication>
#include <QtGui/QKeyEvent>
#include <QtGui/QTextCharFormat>
#include <QtGui/QColor>
#include <QtCore/QCoreApplication>
#include <cstring>

// ── Static callback table ─────────────────────────────────────────────────────

const Y2skkCallbacks Y2skkInputContext::s_callbacks = {
    .commit          = Y2skkInputContext::cbCommit,
    .update_preedit  = Y2skkInputContext::cbUpdatePreedit,
    .clear_preedit   = Y2skkInputContext::cbClearPreedit,
    .show_candidates = Y2skkInputContext::cbShowCandidates,
    .hide_candidates = Y2skkInputContext::cbHideCandidates,
    .update_status   = Y2skkInputContext::cbUpdateStatus,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

static QObject *focused_object()
{
    return QGuiApplication::focusObject();
}

static void send_im_event(QInputMethodEvent &ev)
{
    if (QObject *obj = focused_object())
        QCoreApplication::sendEvent(obj, &ev);
}

// ── Construction / destruction ────────────────────────────────────────────────

Y2skkInputContext::Y2skkInputContext()
{
    // y2skk_init is idempotent (OnceLock); safe to call on every construction.
    if (y2skk_init() != 0)
        return; // leaves m_session_id == 0; isValid() will return false
    m_session_id = y2skk_create_session("qt6");
}

Y2skkInputContext::~Y2skkInputContext()
{
    if (m_session_id != 0) {
        y2skk_destroy_session(m_session_id);
        m_session_id = 0;
    }
}

bool Y2skkInputContext::isValid() const
{
    return m_session_id != 0;
}

// ── Qt event handling ─────────────────────────────────────────────────────────

bool Y2skkInputContext::filterEvent(const QEvent *event)
{
    if (m_session_id == 0)
        return false;

    if (event->type() != QEvent::KeyPress && event->type() != QEvent::KeyRelease)
        return false;

    const auto *key_event = static_cast<const QKeyEvent *>(event);
    const int   is_press  = (event->type() == QEvent::KeyPress) ? 1 : 0;

    const uint32_t qt_key  = static_cast<uint32_t>(key_event->key());
    const uint32_t qt_mods = static_cast<uint32_t>(key_event->modifiers());

    int consumed = y2skk_process_key(
        m_session_id,
        qt_key,
        qt_mods,
        is_press,
        this,
        &s_callbacks
    );

    return consumed != 0;
}

void Y2skkInputContext::setFocusObject(QObject *object)
{
    const bool now_focused = (object != nullptr);

    if (now_focused && !m_has_focus) {
        m_has_focus = true;
        if (m_session_id != 0)
            y2skk_focus_in(m_session_id);
    } else if (!now_focused && m_has_focus) {
        m_has_focus = false;
        if (m_session_id != 0)
            y2skk_focus_out(m_session_id);
    }

    QPlatformInputContext::setFocusObject(object);
}

void Y2skkInputContext::reset()
{
    QPlatformInputContext::reset();
    if (m_session_id != 0)
        y2skk_reset(m_session_id);
}

// ── Callback implementations ──────────────────────────────────────────────────

void Y2skkInputContext::cbCommit(void *ctx, const char *text)
{
    auto *self = static_cast<Y2skkInputContext *>(ctx);

    // Clear any active preedit before committing.
    if (!self->m_preedit.isEmpty()) {
        self->m_preedit.clear();
        QInputMethodEvent clear_ev;
        send_im_event(clear_ev);
    }

    // Commit the text.
    QInputMethodEvent ev;
    ev.setCommitString(QString::fromUtf8(text));
    send_im_event(ev);

    // The cursor has moved; keep the status window aligned if visible.
    if (self->m_status)
        self->m_status->followCursor();
}

void Y2skkInputContext::cbUpdatePreedit(void *ctx, const char *text, uint32_t cursor,
                                        uint32_t ghost_start)
{
    auto *self = static_cast<Y2skkInputContext *>(ctx);

    // Keep the status window aligned as the preedit cursor moves.
    if (self->m_status)
        self->m_status->followCursor();

    self->m_preedit = QString::fromUtf8(text);

    QList<QInputMethodEvent::Attribute> attrs;

    const bool has_ghost = (ghost_start != UINT32_MAX) && (ghost_start < static_cast<uint32_t>(strlen(text)));

    if (has_ghost) {
        // User-typed portion: underline only (0..ghost_start in char units).
        const int char_ghost = QString::fromUtf8(text, static_cast<int>(ghost_start)).length();

        if (char_ghost > 0) {
            QTextCharFormat input_fmt;
            input_fmt.setUnderlineStyle(QTextCharFormat::SingleUnderline);
            attrs.append(QInputMethodEvent::Attribute(
                QInputMethodEvent::TextFormat,
                0,
                char_ghost,
                input_fmt
            ));
        }

        // Ghost portion: grey foreground, no underline (char_ghost..end).
        const int ghost_len = self->m_preedit.length() - char_ghost;
        if (ghost_len > 0) {
            QTextCharFormat ghost_fmt;
            ghost_fmt.setForeground(QColor(0x80, 0x80, 0x80));
            attrs.append(QInputMethodEvent::Attribute(
                QInputMethodEvent::TextFormat,
                char_ghost,
                ghost_len,
                ghost_fmt
            ));
        }
    } else {
        // No ghost: underline everything.
        QTextCharFormat fmt;
        fmt.setUnderlineStyle(QTextCharFormat::SingleUnderline);
        attrs.append(QInputMethodEvent::Attribute(
            QInputMethodEvent::TextFormat,
            0,
            self->m_preedit.length(),
            fmt
        ));
    }

    // Cursor position: Rust gives byte offset, Qt wants QChar index.
    const int char_cursor = QString::fromUtf8(text, static_cast<int>(cursor)).length();
    attrs.append(QInputMethodEvent::Attribute(
        QInputMethodEvent::Cursor,
        char_cursor,
        1,
        {}
    ));

    QInputMethodEvent ev(self->m_preedit, attrs);
    send_im_event(ev);
}

void Y2skkInputContext::cbClearPreedit(void *ctx)
{
    auto *self = static_cast<Y2skkInputContext *>(ctx);
    self->m_preedit.clear();
    QInputMethodEvent ev;
    send_im_event(ev);
}

void Y2skkInputContext::cbShowCandidates(void *ctx, const char **words, uint32_t focused, const char *keys)
{
    auto *self = static_cast<Y2skkInputContext *>(ctx);

    if (!self->m_candidates)
        self->m_candidates = new CandidateWindow();

    QStringList list;
    for (const char **p = words; *p != nullptr; ++p)
        list.append(QString::fromUtf8(*p));

    self->m_candidates->setSelectionKeys(QString::fromUtf8(keys));
    self->m_candidates->updateCandidates(list);

    // Position the window only on first show; subsequent calls just repaint.
    if (!self->m_candidates->isVisible())
        self->m_candidates->showAtCursor();
}

void Y2skkInputContext::cbHideCandidates(void *ctx)
{
    auto *self = static_cast<Y2skkInputContext *>(ctx);
    if (self->m_candidates)
        self->m_candidates->hide();
}

void Y2skkInputContext::cbUpdateStatus(void *ctx, const char *indicator)
{
    auto *self = static_cast<Y2skkInputContext *>(ctx);

    if (!self->m_status)
        self->m_status = new StatusWindow();

    self->m_status->showWithIndicator(QString::fromUtf8(indicator));
}
