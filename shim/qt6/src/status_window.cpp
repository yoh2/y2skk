/*
 * status_window.cpp — Floating input-mode indicator popup implementation.
 */

#include "status_window.h"

#include <QCoreApplication>
#include <QGuiApplication>
#include <QInputMethod>
#include <QInputMethodQueryEvent>
#include <QPainter>
#include <QScreen>
#include <QWindow>

// ── Construction ──────────────────────────────────────────────────────────────

StatusWindow::StatusWindow()
{
    // ToolTip: frameless, does not steal focus, stays above normal windows.
    setFlags(Qt::ToolTip | Qt::FramelessWindowHint | Qt::WindowDoesNotAcceptFocus);

    m_hide_timer = new QTimer(this);
    m_hide_timer->setSingleShot(true);
    connect(m_hide_timer, &QTimer::timeout, this, &StatusWindow::hide);
}

// ── Public interface ──────────────────────────────────────────────────────────

void StatusWindow::showWithIndicator(const QString &indicator)
{
    m_indicator = indicator;
    recalcSize();
    positionAtCursor();

    // Hide first if already visible so the re-show triggers a fresh expose event,
    // which guarantees a repaint with the updated indicator text.
    if (isVisible())
        setVisible(false);
    setVisible(true);

    // Restart the auto-hide timer.
    m_hide_timer->start(kHideDelayMs);
}

void StatusWindow::followCursor()
{
    if (isVisible())
        positionAtCursor();
}

// ── Private helpers ───────────────────────────────────────────────────────────

void StatusWindow::recalcSize()
{
    QFont f = QGuiApplication::font();
    QFontMetrics fm(f);

    int w = fm.horizontalAdvance(m_indicator) + 2 * kPad;
    int h = fm.height() + 2 * kPad;

    // Enforce a minimum square size.
    int side = qMax(w, h);
    resize(side, side);
}

void StatusWindow::positionAtCursor()
{
    static constexpr int kGap = 4;

    QPoint screenBottom;
    QWindow *focusWin = QGuiApplication::focusWindow();
    if (focusWin) {
        QObject *focusObj = focusWin->focusObject();
        if (focusObj) {
            QInputMethodQueryEvent q(Qt::ImCursorRectangle);
            QCoreApplication::sendEvent(focusObj, &q);
            QRectF itemRect = q.value(Qt::ImCursorRectangle).toRectF();

            QTransform t = QGuiApplication::inputMethod()->inputItemTransform();
            QRectF winRect = t.mapRect(itemRect);

            if (winRect.height() < 1.0)
                winRect.setHeight(QFontMetrics(QGuiApplication::font()).height());

            screenBottom = focusWin->mapToGlobal(winRect.bottomLeft().toPoint());
        }
    }

    QPoint screenPos(screenBottom.x(), screenBottom.y() + kGap);

    QScreen *scr = focusWin ? focusWin->screen() : nullptr;
    if (!scr)
        scr = QGuiApplication::primaryScreen();

    if (scr) {
        QRect avail = scr->availableGeometry();

        if (screenPos.x() + width() > avail.right())
            screenPos.setX(avail.right() - width());
        screenPos.setX(qMax(screenPos.x(), avail.left()));

        if (screenPos.y() + height() > avail.bottom())
            screenPos.setY(screenPos.y() - height() - kGap * 2);
        screenPos.setY(qMax(screenPos.y(), avail.top()));
    }

    setPosition(screenPos);
}

// ── Painting ──────────────────────────────────────────────────────────────────

void StatusWindow::paintEvent(QPaintEvent *)
{
    QPainter p(this);

    // Background + border
    QRect winRect(0, 0, width(), height());
    p.fillRect(winRect, QColor(0xfe, 0xfe, 0xfe));
    p.setPen(QColor(0x88, 0x88, 0x88));
    p.drawRect(winRect.adjusted(0, 0, -1, -1));

    // Indicator character — normal font, dark colour
    QFont f = QGuiApplication::font();
    p.setFont(f);
    p.setPen(QColor(0x10, 0x10, 0x10));
    p.drawText(winRect, Qt::AlignCenter, m_indicator);
}
