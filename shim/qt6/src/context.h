/*
 * context.h — Y2skkInputContext declaration.
 */

#pragma once

#include <QtGui/qpa/qplatforminputcontext.h>
#include <QtCore/QString>
#include <QtCore/QList>

#include "rust_bridge.h"
#include "candidate_window.h"
#include "status_window.h"

class Y2skkInputContext : public QPlatformInputContext
{
    Q_OBJECT

public:
    explicit Y2skkInputContext();
    ~Y2skkInputContext() override;

    bool isValid() const override;

    bool filterEvent(const QEvent *event) override;
    // Called when the focused QObject changes (replaces Qt5 focusIn/focusOut).
    void setFocusObject(QObject *object) override;
    void reset() override;
    void update(Qt::InputMethodQueries) override {}

    QString preeditText() const { return m_preedit; }

private:
    uint32_t        m_session_id  = 0;
    bool            m_has_focus   = false;
    QString         m_preedit;
    CandidateWindow *m_candidates = nullptr;
    StatusWindow    *m_status     = nullptr;

    static void cbCommit(void *ctx, const char *text);
    static void cbUpdatePreedit(void *ctx, const char *text, uint32_t cursor, uint32_t ghost_start);
    static void cbClearPreedit(void *ctx);
    static void cbShowCandidates(void *ctx, const char **words, uint32_t focused, const char *keys);
    static void cbHideCandidates(void *ctx);
    static void cbUpdateStatus(void *ctx, const char *indicator);

    static const Y2skkCallbacks s_callbacks;
};
