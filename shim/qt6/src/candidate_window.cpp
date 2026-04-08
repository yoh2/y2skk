/*
 * candidate_window.cpp — Floating candidate list popup implementation.
 */

#include "candidate_window.h"

#include <QCoreApplication>
#include <QGuiApplication>
#include <QInputMethod>
#include <QInputMethodQueryEvent>
#include <QPainter>
#include <QScreen>
#include <QWindow>

// ── Construction ──────────────────────────────────────────────────────────────

CandidateWindow::CandidateWindow()
{
    // ToolTip: frameless, does not steal focus, stays above normal windows.
    setFlags(Qt::ToolTip | Qt::FramelessWindowHint | Qt::WindowDoesNotAcceptFocus);
}

// ── Public interface ──────────────────────────────────────────────────────────

void CandidateWindow::updateCandidates(const QStringList &words)
{
    m_words = words;

    if (words.isEmpty()) {
        hide();
        return;
    }

    recalcSize();
    requestUpdate();
}

void CandidateWindow::showAtCursor()
{
    static constexpr int kGap = 4; // pixels between cursor and popup

    // Query the cursor rectangle from the focused input object.
    // ImCursorRectangle is in input-item (widget) coordinates; use
    // inputItemTransform() to convert to window coordinates first, then
    // mapToGlobal() to reach screen coordinates.
    QPoint screenTop, screenBottom;
    QWindow *focusWin = QGuiApplication::focusWindow();
    if (focusWin) {
        QObject *focusObj = focusWin->focusObject();
        if (focusObj) {
            QInputMethodQueryEvent q(Qt::ImCursorRectangle);
            QCoreApplication::sendEvent(focusObj, &q);
            QRectF itemRect = q.value(Qt::ImCursorRectangle).toRectF();

            // Map from input-item coordinates to window coordinates.
            QTransform t = QGuiApplication::inputMethod()->inputItemTransform();
            QRectF winRect = t.mapRect(itemRect);

            // If the rect has no height, estimate from font metrics so the
            // popup is placed at least one line below the text.
            if (winRect.height() < 1.0)
                winRect.setHeight(QFontMetrics(QGuiApplication::font()).height());

            screenTop    = focusWin->mapToGlobal(winRect.topLeft().toPoint());
            screenBottom = focusWin->mapToGlobal(winRect.bottomLeft().toPoint());
        }
    }

    // Start below the cursor with a small gap.
    QPoint screenPos(screenBottom.x(), screenBottom.y() + kGap);

    // Use the screen the focus window is on (not necessarily the primary screen).
    QScreen *scr = focusWin ? focusWin->screen() : nullptr;
    if (!scr)
        scr = QGuiApplication::primaryScreen();

    if (scr) {
        QRect avail = scr->availableGeometry();

        // Clamp horizontally.
        if (screenPos.x() + width() > avail.right())
            screenPos.setX(avail.right() - width());
        screenPos.setX(qMax(screenPos.x(), avail.left()));

        // If the popup would extend below the screen, flip it above the cursor.
        if (screenPos.y() + height() > avail.bottom())
            screenPos.setY(screenTop.y() - height() - kGap);
        screenPos.setY(qMax(screenPos.y(), avail.top()));
    }

    setPosition(screenPos);
    setVisible(true);
}

// ── Private helpers ───────────────────────────────────────────────────────────

void CandidateWindow::recalcSize()
{
    QFont f = QGuiApplication::font();
    QFontMetrics fm(f);
    QFont keyFont = f;
    keyFont.setBold(true);
    QFontMetrics fmKey(keyFont);

    int maxW = kMinWidth;
    int visibleRows = qMin(m_words.size(), kMaxVisible);

    for (int i = 0; i < visibleRows; ++i) {
        const QString &raw = m_words[i];
        int semi = raw.indexOf(';');
        QString word = (semi >= 0) ? raw.left(semi) : raw;
        QString ann  = (semi >= 0) ? raw.mid(semi + 1) : QString();

        // key label + separator + word
        QString label = (i < m_keys.size()) ? QString(m_keys[i]) + " " : QString();
        int w = fmKey.horizontalAdvance(label) + fm.horizontalAdvance(word) + 2 * kHPad;
        if (!ann.isEmpty())
            w += fm.horizontalAdvance("  " + ann);
        maxW = qMax(maxW, w);
    }

    int h = visibleRows * kLineHeight + 2 * kVPad;
    resize(maxW + 2, h + 2);
}

// ── Painting ──────────────────────────────────────────────────────────────────

void CandidateWindow::paintEvent(QPaintEvent *)
{
    QPainter p(this);

    // Background + border
    QRect winRect(0, 0, width(), height());
    p.fillRect(winRect, QColor(0xfe, 0xfe, 0xfe));
    p.setPen(QColor(0x88, 0x88, 0x88));
    p.drawRect(winRect.adjusted(0, 0, -1, -1));

    QFont f = QGuiApplication::font();
    QFont keyFont = f;
    keyFont.setBold(true);
    QFont annFont = f;
    annFont.setPointSizeF(f.pointSizeF() * 0.85);
    QFontMetrics fm(f);
    QFontMetrics fmKey(keyFont);

    int visibleRows = qMin(m_words.size(), kMaxVisible);

    for (int row = 0; row < visibleRows; ++row) {
        int y = kVPad + 1 + row * kLineHeight;
        QRect rowRect(1, y, width() - 2, kLineHeight);

        const QString &raw = m_words[row];
        int semi = raw.indexOf(';');
        QString word = (semi >= 0) ? raw.left(semi) : raw;
        QString ann  = (semi >= 0) ? raw.mid(semi + 1) : QString();

        int x = rowRect.left() + kHPad;

        // Selection key label (bold, accent colour)
        if (row < m_keys.size()) {
            QString label = QString(m_keys[row]) + " ";
            p.setFont(keyFont);
            p.setPen(QColor(0x50, 0x90, 0xd0));
            QRect keyRect(x, y, fmKey.horizontalAdvance(label), kLineHeight);
            p.drawText(keyRect, Qt::AlignVCenter | Qt::AlignLeft, label);
            x += fmKey.horizontalAdvance(label);
        }

        // Word text
        p.setFont(f);
        p.setPen(QColor(0x10, 0x10, 0x10));
        QRect wordRect(x, y, rowRect.right() - x, kLineHeight);
        p.drawText(wordRect, Qt::AlignVCenter | Qt::AlignLeft, word);

        // Annotation (dimmed, smaller font)
        if (!ann.isEmpty()) {
            int wordW = fm.horizontalAdvance(word);
            p.setFont(annFont);
            p.setPen(QColor(0x70, 0x70, 0x70));
            QRect annRect(x + wordW + kHPad, y,
                          rowRect.right() - x - wordW - kHPad,
                          kLineHeight);
            p.drawText(annRect, Qt::AlignVCenter | Qt::AlignLeft, ann);
        }
    }
}
