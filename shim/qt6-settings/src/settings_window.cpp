#include "settings_window.h"

#include "y2skk_settings.h"

#include <QtWidgets>

#include <QFutureWatcher>
#include <QtConcurrent>

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonParseError>
#include <QJsonValue>

namespace {

// Joins the trimmed, non-empty lines of a multi-line editor into a JSON array.
QJsonArray linesToArray(const QPlainTextEdit *edit)
{
    QJsonArray arr;
    const QStringList lines = edit->toPlainText().split(QLatin1Char('\n'));
    for (const QString &raw : lines) {
        const QString s = raw.trimmed();
        if (!s.isEmpty()) {
            arr.append(s);
        }
    }
    return arr;
}

// Fills a multi-line editor with one array element per line.
void setLines(QPlainTextEdit *edit, const QJsonArray &arr)
{
    QStringList lines;
    for (const QJsonValue &v : arr) {
        lines << v.toString();
    }
    edit->setPlainText(lines.join(QLatin1Char('\n')));
}

// Collapses the hard-wrapped lines within each blank-line-separated paragraph
// into one line, so a word-wrapping view can reflow the text to its own width
// instead of mixing the source's fixed-width breaks with the widget's soft
// wrapping.  Blank lines are kept as paragraph separators.
QString reflowParagraphs(const QString &text)
{
    QStringList paragraphs;
    for (const QString &para : text.split(QStringLiteral("\n\n"))) {
        QStringList lines;
        for (const QString &line : para.split(QLatin1Char('\n'))) {
            const QString trimmed = line.trimmed();
            if (!trimmed.isEmpty()) {
                lines << trimmed;
            }
        }
        paragraphs << lines.join(QLatin1Char(' '));
    }
    return paragraphs.join(QStringLiteral("\n\n"));
}

// Calls a Rust FFI getter and returns the result as a QString, freeing the
// owned buffer.
QString takeString(char *owned)
{
    const QString s = QString::fromUtf8(owned ? owned : "");
    y2skk_settings_string_free(owned);
    return s;
}

} // namespace

SettingsWindow::SettingsWindow(QWidget *parent)
    : QMainWindow(parent)
{
    setWindowTitle(tr("y2skk Settings"));

    auto *tabs = new QTabWidget;
    tabs->addTab(buildInputTab(), tr("Input"));
    tabs->addTab(buildDictionariesTab(), tr("Dictionaries"));
    tabs->addTab(buildCandidatesTab(), tr("Candidates"));
    tabs->addTab(buildIndicatorTab(), tr("Indicator"));
    tabs->addTab(buildAdvancedTab(), tr("Advanced"));
    tabs->addTab(buildAboutTab(), tr("About"));

    auto *pathLabel = new QLabel(
        tr("Config: %1").arg(takeString(y2skk_settings_default_config_path())));
    pathLabel->setStyleSheet(QStringLiteral("color: gray;"));

    m_status = new QLabel;
    m_status->setWordWrap(true);

    m_applyButton = new QPushButton(tr("Apply"));
    auto *closeBtn = new QPushButton(tr("Close"));
    connect(m_applyButton, &QPushButton::clicked, this, &SettingsWindow::onApply);
    connect(closeBtn, &QPushButton::clicked, this, &QWidget::close);

    auto *btnRow = new QHBoxLayout;
    btnRow->addWidget(m_status, 1);
    btnRow->addWidget(m_applyButton);
    btnRow->addWidget(closeBtn);

    auto *central = new QWidget;
    auto *layout = new QVBoxLayout(central);
    layout->addWidget(tabs, 1);
    layout->addWidget(pathLabel);
    layout->addLayout(btnRow);
    setCentralWidget(central);

    loadFromCore();
    resize(560, 540);
}

QWidget *SettingsWindow::buildInputTab()
{
    auto *w = new QWidget;
    auto *form = new QFormLayout(w);

    m_kanaLayout = new QComboBox;
    m_kanaLayout->setEditable(true);
    form->addRow(tr("Kana layout:"), m_kanaLayout);

    m_kanaTable = new QLineEdit;
    auto *tableRow = new QHBoxLayout;
    tableRow->addWidget(m_kanaTable, 1);
    auto *tableBrowse = new QPushButton(tr("Browse…"));
    connect(tableBrowse, &QPushButton::clicked, this, [this] {
        const QString f = QFileDialog::getOpenFileName(this, tr("Select kana table"));
        if (!f.isEmpty()) {
            m_kanaTable->setText(f);
        }
    });
    tableRow->addWidget(tableBrowse);
    form->addRow(tr("Custom table (optional):"), tableRow);

    m_defaultMode = new QComboBox;
    m_defaultMode->addItems({QStringLiteral("ascii"), QStringLiteral("hiragana"),
                             QStringLiteral("katakana"),
                             QStringLiteral("half-width-katakana"),
                             QStringLiteral("wide-ascii")});
    form->addRow(tr("Default mode:"), m_defaultMode);

    m_viEscape = new QCheckBox(tr("Esc switches to ASCII in normal input modes (vi_escape)"));
    form->addRow(QString(), m_viEscape);

    m_toggleKeys = new QPlainTextEdit;
    m_toggleKeys->setPlaceholderText(tr("one key combination per line, e.g. shift+space"));
    m_toggleKeys->setFixedHeight(52);
    form->addRow(tr("IME toggle keys:"), m_toggleKeys);

    m_rawToggleKeys = new QPlainTextEdit;
    m_rawToggleKeys->setPlaceholderText(tr("one key combination per line, e.g. ctrl+alt+p"));
    m_rawToggleKeys->setFixedHeight(52);
    form->addRow(tr("Raw-mode toggle keys:"), m_rawToggleKeys);

    m_triggerChars = new QPlainTextEdit;
    m_triggerChars->setPlaceholderText(tr("one character per line, e.g. ,"));
    m_triggerChars->setFixedHeight(52);
    form->addRow(tr("Conversion triggers:"), m_triggerChars);

    return w;
}

QWidget *SettingsWindow::buildDictionariesTab()
{
    auto *w = new QWidget;
    auto *layout = new QVBoxLayout(w);

    m_dictSources = new QTableWidget(0, 3);
    m_dictSources->setHorizontalHeaderLabels({tr("Path"), tr("Encoding"), tr("Priority")});
    m_dictSources->horizontalHeader()->setSectionResizeMode(0, QHeaderView::Stretch);
    m_dictSources->setSelectionBehavior(QAbstractItemView::SelectRows);
    layout->addWidget(m_dictSources, 1);

    auto *btns = new QHBoxLayout;
    auto *add = new QPushButton(tr("Add…"));
    auto *del = new QPushButton(tr("Remove"));
    connect(add, &QPushButton::clicked, this, [this] {
        const QString f = QFileDialog::getOpenFileName(this, tr("Select dictionary file"));
        if (!f.isEmpty()) {
            addDictRow(f, QStringLiteral("euc-jp"), 0);
        }
    });
    connect(del, &QPushButton::clicked, this, [this] {
        const int r = m_dictSources->currentRow();
        if (r >= 0) {
            m_dictSources->removeRow(r);
        }
    });
    btns->addStretch(1);
    btns->addWidget(add);
    btns->addWidget(del);
    layout->addLayout(btns);

    auto *form = new QFormLayout;
    m_userDict = new QLineEdit;
    auto *udRow = new QHBoxLayout;
    udRow->addWidget(m_userDict, 1);
    auto *udBrowse = new QPushButton(tr("Browse…"));
    connect(udBrowse, &QPushButton::clicked, this, [this] {
        const QString f = QFileDialog::getSaveFileName(this, tr("Select user dictionary"));
        if (!f.isEmpty()) {
            m_userDict->setText(f);
        }
    });
    udRow->addWidget(udBrowse);
    form->addRow(tr("User dictionary (optional):"), udRow);
    layout->addLayout(form);

    m_lispDirectives =
        new QCheckBox(tr("Honour (skk-ignore-dic-word …) directives (lisp_directives)"));
    layout->addWidget(m_lispDirectives);

    return w;
}

QWidget *SettingsWindow::buildCandidatesTab()
{
    auto *w = new QWidget;
    auto *form = new QFormLayout(w);

    m_inlineCount = new QSpinBox;
    // 0 is valid (immediate listing mode); use a generous upper bound so an
    // existing value is never silently clamped (and rewritten) on Apply.
    m_inlineCount->setRange(0, 1000000);
    form->addRow(tr("Inline candidate count:"), m_inlineCount);

    m_selectionKeys = new QLineEdit;
    form->addRow(tr("Selection keys:"), m_selectionKeys);

    return w;
}

QWidget *SettingsWindow::buildIndicatorTab()
{
    auto *w = new QWidget;
    auto *form = new QFormLayout(w);

    m_indicatorEnabled = new QCheckBox(tr("Show the mode indicator"));
    form->addRow(QString(), m_indicatorEnabled);

    m_indicatorTimeout = new QSpinBox;
    m_indicatorTimeout->setRange(0, 60000);
    m_indicatorTimeout->setSingleStep(100);
    m_indicatorTimeout->setSuffix(tr(" ms"));
    form->addRow(tr("Auto-hide timeout (0 = always visible):"), m_indicatorTimeout);

    return w;
}

QWidget *SettingsWindow::buildAdvancedTab()
{
    auto *w = new QWidget;
    auto *form = new QFormLayout(w);

    m_logLevel = new QComboBox;
    m_logLevel->setEditable(true);
    m_logLevel->addItems({QStringLiteral("error"), QStringLiteral("warn"),
                          QStringLiteral("info"), QStringLiteral("debug"),
                          QStringLiteral("trace")});
    form->addRow(tr("Daemon log level:"), m_logLevel);

    auto *note = new QLabel(tr("Note: the log level takes effect the next time the daemon starts."));
    note->setWordWrap(true);
    note->setStyleSheet(QStringLiteral("color: gray;"));
    form->addRow(QString(), note);

    return w;
}

QWidget *SettingsWindow::buildAboutTab()
{
    auto *w = new QWidget;
    auto *layout = new QVBoxLayout(w);

    auto *title =
        new QLabel(tr("y2skk Settings %1").arg(takeString(y2skk_settings_version())));
    title->setStyleSheet(QStringLiteral("font-weight: bold; font-size: 14pt;"));
    layout->addWidget(title);

    auto *desc = new QLabel(tr("A Qt6 settings GUI for the y2skk IME.  (Qt runtime %1)")
                                .arg(QString::fromLatin1(qVersion())));
    desc->setWordWrap(true);
    layout->addWidget(desc);

    layout->addWidget(new QLabel(tr("License")));

    auto *license = new QPlainTextEdit;
    license->setReadOnly(true);
    license->setPlainText(reflowParagraphs(takeString(y2skk_settings_license_text())));
    layout->addWidget(license, 1); // license box takes the remaining space

    return w;
}

void SettingsWindow::addDictRow(const QString &path, const QString &encoding, int priority)
{
    const int r = m_dictSources->rowCount();
    m_dictSources->insertRow(r);
    m_dictSources->setItem(r, 0, new QTableWidgetItem(path));

    auto *encCombo = new QComboBox;
    encCombo->addItems({QStringLiteral("utf-8"), QStringLiteral("euc-jp")});
    encCombo->setCurrentText(encoding.isEmpty() ? QStringLiteral("utf-8") : encoding);
    m_dictSources->setCellWidget(r, 1, encCombo);

    // Use a spin box so the priority is always a valid integer (a free-text
    // cell would silently coerce non-numeric input to 0).
    auto *prioSpin = new QSpinBox;
    prioSpin->setRange(-100000, 100000);
    prioSpin->setValue(priority);
    m_dictSources->setCellWidget(r, 2, prioSpin);
}

void SettingsWindow::loadFromCore()
{
    // Populate the kana layout choices discovered from the table directories.
    QJsonParseError perr{};
    const QJsonArray layouts =
        QJsonDocument::fromJson(takeString(y2skk_settings_kana_layouts_json()).toUtf8(), &perr)
            .array();
    if (perr.error != QJsonParseError::NoError) {
        m_status->setText(tr("✗ Internal error reading the layout list: %1").arg(perr.errorString()));
        m_status->setStyleSheet(QStringLiteral("color: red;"));
    }
    m_kanaLayout->clear();
    for (const QJsonValue &v : layouts) {
        m_kanaLayout->addItem(v.toString());
    }

    QJsonParseError rerr{};
    const QJsonObject root =
        QJsonDocument::fromJson(takeString(y2skk_settings_load_json()).toUtf8(), &rerr).object();
    if (rerr.error != QJsonParseError::NoError) {
        m_status->setText(tr("✗ Internal error reading configuration: %1").arg(rerr.errorString()));
        m_status->setStyleSheet(QStringLiteral("color: red;"));
        // Don't allow applying partially-initialized/default widget values over
        // the user's config when the load failed.
        m_applyButton->setEnabled(false);
        return; // cannot populate the form from unparseable data
    }

    const QJsonObject in = root.value(QStringLiteral("input")).toObject();
    const QString layout = in.value(QStringLiteral("kana_layout")).toString(QStringLiteral("romaji"));
    if (m_kanaLayout->findText(layout) < 0) {
        m_kanaLayout->addItem(layout);
    }
    m_kanaLayout->setCurrentText(layout);
    m_kanaTable->setText(in.value(QStringLiteral("kana_table")).toString());
    // default_mode accepts aliases (e.g. "latin", "zenkaku") the combo doesn't
    // list; add an unknown value so it isn't silently coerced to "ascii".
    const QString mode = in.value(QStringLiteral("default_mode")).toString(QStringLiteral("ascii"));
    if (m_defaultMode->findText(mode) < 0) {
        m_defaultMode->addItem(mode);
    }
    m_defaultMode->setCurrentText(mode);
    m_viEscape->setChecked(in.value(QStringLiteral("vi_escape")).toBool(false));
    setLines(m_toggleKeys, in.value(QStringLiteral("toggle_keys")).toArray());
    setLines(m_rawToggleKeys, in.value(QStringLiteral("raw_mode_toggle_keys")).toArray());
    setLines(m_triggerChars, in.value(QStringLiteral("conversion_trigger_chars")).toArray());

    m_userDict->setText(
        root.value(QStringLiteral("user-dict")).toObject().value(QStringLiteral("path")).toString());

    const QJsonObject dict = root.value(QStringLiteral("dict")).toObject();
    m_dictSources->setRowCount(0);
    for (const QJsonValue &v : dict.value(QStringLiteral("sources")).toArray()) {
        const QJsonObject s = v.toObject();
        addDictRow(s.value(QStringLiteral("path")).toString(),
                   s.value(QStringLiteral("encoding")).toString(QStringLiteral("utf-8")),
                   s.value(QStringLiteral("priority")).toInt(0));
    }
    m_lispDirectives->setChecked(dict.value(QStringLiteral("lisp_directives")).toBool(true));

    const QJsonObject ind = root.value(QStringLiteral("indicator")).toObject();
    m_indicatorEnabled->setChecked(ind.value(QStringLiteral("enabled")).toBool(true));
    m_indicatorTimeout->setValue(ind.value(QStringLiteral("timeout_ms")).toInt(2000));

    const QJsonObject cand = root.value(QStringLiteral("candidates")).toObject();
    m_inlineCount->setValue(cand.value(QStringLiteral("inline_count")).toInt(3));
    m_selectionKeys->setText(
        cand.value(QStringLiteral("selection_keys")).toString(QStringLiteral("asdfjkl;")));

    m_logLevel->setCurrentText(root.value(QStringLiteral("daemon"))
                                   .toObject()
                                   .value(QStringLiteral("log_level"))
                                   .toString(QStringLiteral("info")));

    // If the on-disk config exists but is invalid TOML, the values above are
    // defaults; warn the user (Apply will refuse to overwrite an invalid file).
    const QString loadErr = takeString(y2skk_settings_load_error());
    if (!loadErr.isEmpty()) {
        m_status->setText(tr("⚠ Showing defaults — the config file is invalid: %1").arg(loadErr));
        m_status->setStyleSheet(QStringLiteral("color: #b58900;"));
    }
}

QByteArray SettingsWindow::toJson() const
{
    QJsonObject input;
    input[QStringLiteral("kana_layout")] = m_kanaLayout->currentText().trimmed();
    const QString table = m_kanaTable->text().trimmed();
    input[QStringLiteral("kana_table")] = table.isEmpty() ? QJsonValue(QJsonValue::Null) : QJsonValue(table);
    input[QStringLiteral("default_mode")] = m_defaultMode->currentText();
    input[QStringLiteral("vi_escape")] = m_viEscape->isChecked();
    input[QStringLiteral("toggle_keys")] = linesToArray(m_toggleKeys);
    input[QStringLiteral("raw_mode_toggle_keys")] = linesToArray(m_rawToggleKeys);
    input[QStringLiteral("conversion_trigger_chars")] = linesToArray(m_triggerChars);

    QJsonObject userDict;
    const QString ud = m_userDict->text().trimmed();
    userDict[QStringLiteral("path")] = ud.isEmpty() ? QJsonValue(QJsonValue::Null) : QJsonValue(ud);

    QJsonArray sources;
    for (int r = 0; r < m_dictSources->rowCount(); ++r) {
        const QTableWidgetItem *pathItem = m_dictSources->item(r, 0);
        const QString path = pathItem ? pathItem->text().trimmed() : QString();
        if (path.isEmpty()) {
            continue;
        }
        const auto *encCombo = qobject_cast<QComboBox *>(m_dictSources->cellWidget(r, 1));
        const auto *prioSpin = qobject_cast<QSpinBox *>(m_dictSources->cellWidget(r, 2));
        QJsonObject src;
        src[QStringLiteral("path")] = path;
        src[QStringLiteral("encoding")] =
            encCombo ? encCombo->currentText() : QStringLiteral("utf-8");
        src[QStringLiteral("priority")] = prioSpin ? prioSpin->value() : 0;
        sources.append(src);
    }
    QJsonObject dict;
    dict[QStringLiteral("sources")] = sources;
    dict[QStringLiteral("lisp_directives")] = m_lispDirectives->isChecked();

    QJsonObject indicator;
    indicator[QStringLiteral("enabled")] = m_indicatorEnabled->isChecked();
    indicator[QStringLiteral("timeout_ms")] = m_indicatorTimeout->value();

    QJsonObject candidates;
    candidates[QStringLiteral("inline_count")] = m_inlineCount->value();
    candidates[QStringLiteral("selection_keys")] = m_selectionKeys->text();

    QJsonObject daemon;
    daemon[QStringLiteral("log_level")] = m_logLevel->currentText().trimmed();

    QJsonObject root;
    root[QStringLiteral("input")] = input;
    root[QStringLiteral("user-dict")] = userDict;
    root[QStringLiteral("dict")] = dict;
    root[QStringLiteral("indicator")] = indicator;
    root[QStringLiteral("candidates")] = candidates;
    root[QStringLiteral("daemon")] = daemon;

    return QJsonDocument(root).toJson(QJsonDocument::Compact);
}

void SettingsWindow::onApply()
{
    // apply_json validates, writes config.toml and performs a blocking D-Bus
    // reload, which can be slow.  Run it on a worker thread so the UI stays
    // responsive; disable Apply while in flight and report on completion.
    const QByteArray json = toJson();
    m_applyButton->setEnabled(false);
    m_status->setText(tr("Applying…"));
    m_status->setStyleSheet(QString());

    auto *watcher = new QFutureWatcher<QString>(this);
    connect(watcher, &QFutureWatcher<QString>::finished, this, [this, watcher] {
        const QString err = watcher->result();
        watcher->deleteLater();
        m_applyButton->setEnabled(true);
        if (err.isEmpty()) {
            m_status->setText(tr("✓ Applied."));
            m_status->setStyleSheet(QStringLiteral("color: green;"));
        } else {
            m_status->setText(tr("✗ %1").arg(err));
            m_status->setStyleSheet(QStringLiteral("color: red;"));
        }
    });
    watcher->setFuture(QtConcurrent::run([json] {
        return takeString(y2skk_settings_apply_json(json.constData()));
    }));
}
