/*
 * status_window.h — Floating input-mode indicator popup.
 *
 * A small frameless, focus-stealing-free QRasterWindow that shows the current
 * SKK input mode as a single character ("あ", "ア", "ｱ", "a", "Ａ").
 * It appears below the text cursor on every mode switch, stays visible for a
 * configurable number of seconds, and follows the cursor while visible.
 */

#pragma once

#include <QRasterWindow>
#include <QTimer>
#include <QString>

class StatusWindow : public QRasterWindow
{
    Q_OBJECT

public:
    explicit StatusWindow();

    // Show the window with the given mode indicator and start the hide timer.
    // Calling this while already visible resets the timer and updates the text.
    void showWithIndicator(const QString &indicator);

    // Reposition the window to the current text cursor if the window is visible.
    // Called whenever the preedit or commit position changes.
    void followCursor();

protected:
    void paintEvent(QPaintEvent *event) override;

private:
    QString  m_indicator;
    QTimer  *m_hide_timer = nullptr;

    // Auto-hide delay in milliseconds.
    static constexpr int kHideDelayMs = 2000;

    // Layout constants (logical pixels).
    static constexpr int kPad = 2;

    void recalcSize();
    void positionAtCursor();
};
