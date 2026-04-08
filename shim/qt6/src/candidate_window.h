/*
 * candidate_window.h — Floating candidate list popup.
 *
 * A frameless, focus-stealing-free QRasterWindow that renders the SKK
 * candidate list in listing mode.  Each row shows a selection key label
 * (e.g. "a"), the candidate word, and an optional annotation.
 */

#pragma once

#include <QRasterWindow>
#include <QStringList>

class CandidateWindow : public QRasterWindow
{
    Q_OBJECT

public:
    explicit CandidateWindow();

    // Replace the candidate list and repaint.  Hides the window when empty.
    void updateCandidates(const QStringList &words);

    // Set the selection key characters shown as labels (e.g. "asdfjkl;").
    void setSelectionKeys(const QString &keys) { m_keys = keys; }

    // Re-position next to the input cursor and make the window visible.
    void showAtCursor();

protected:
    void paintEvent(QPaintEvent *event) override;

private:
    QStringList m_words;
    QString     m_keys;   // selection key characters, one per candidate row

    // Layout constants (logical pixels)
    static constexpr int kLineHeight = 26;
    static constexpr int kHPad       = 8;
    static constexpr int kVPad       = 4;
    static constexpr int kMinWidth   = 120;
    static constexpr int kMaxVisible = 10; // cap visible rows to avoid huge windows

    void recalcSize();
};
