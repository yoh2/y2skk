#include <QApplication>
#include <QLibraryInfo>
#include <QLocale>
#include <QTranslator>

#include "settings_window.h"

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("y2skk-settings-qt6"));

    // Locale-ready: install a translation matching the system locale if one is
    // present.  English is the source language, so when no .qm is found the UI
    // simply stays in English.  Translations are looked up next to the binary
    // (../share/y2skk/translations) and in the Qt translations dir.
    QTranslator translator;
    const QString locale = QLocale::system().name(); // e.g. "ja_JP"
    const QString base = QStringLiteral("y2skk-settings_");
    const QStringList dirs = {
        QCoreApplication::applicationDirPath() + QStringLiteral("/../share/y2skk/translations"),
        QLibraryInfo::path(QLibraryInfo::TranslationsPath),
    };
    for (const QString &dir : dirs) {
        if (translator.load(base + locale, dir)) {
            app.installTranslator(&translator);
            break;
        }
    }

    // Set the display name after the translator is installed so it is localized.
    QApplication::setApplicationDisplayName(QObject::tr("y2skk Settings"));

    SettingsWindow window;
    window.show();
    return app.exec();
}
