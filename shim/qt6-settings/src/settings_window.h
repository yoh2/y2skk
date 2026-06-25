#pragma once

#include <QMainWindow>

class QCheckBox;
class QComboBox;
class QLabel;
class QLineEdit;
class QPlainTextEdit;
class QPushButton;
class QSpinBox;
class QTableWidget;

// Top-level settings window.  Loads the current config from the Rust core,
// presents every config option across tabbed pages, and on "Apply" serialises
// the form back to JSON and asks the core to validate, save and reload.
class SettingsWindow : public QMainWindow
{
    Q_OBJECT

public:
    explicit SettingsWindow(QWidget *parent = nullptr);

private slots:
    void onApply();

private:
    QWidget *buildInputTab();
    QWidget *buildDictionariesTab();
    QWidget *buildCandidatesTab();
    QWidget *buildIndicatorTab();
    QWidget *buildAdvancedTab();
    QWidget *buildAboutTab();

    void loadFromCore();
    QByteArray toJson() const;
    void addDictRow(const QString &path, const QString &encoding, int priority);

    // [input]
    QComboBox *m_kanaLayout = nullptr;
    QLineEdit *m_kanaTable = nullptr;
    QComboBox *m_defaultMode = nullptr;
    QCheckBox *m_viEscape = nullptr;
    QPlainTextEdit *m_toggleKeys = nullptr;
    QPlainTextEdit *m_rawToggleKeys = nullptr;
    QPlainTextEdit *m_triggerChars = nullptr;

    // [user-dict]
    QLineEdit *m_userDict = nullptr;

    // [dict]
    QTableWidget *m_dictSources = nullptr;
    QCheckBox *m_lispDirectives = nullptr;

    // [candidates]
    QSpinBox *m_inlineCount = nullptr;
    QLineEdit *m_selectionKeys = nullptr;

    // [indicator]
    QCheckBox *m_indicatorEnabled = nullptr;
    QSpinBox *m_indicatorTimeout = nullptr;

    // [daemon]
    QComboBox *m_logLevel = nullptr;

    QPushButton *m_applyButton = nullptr;
    QLabel *m_status = nullptr;
};
