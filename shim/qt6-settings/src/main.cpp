#include <QApplication>
#include <QLibraryInfo>
#include <QLocale>
#include <QTranslator>

#include <cstdio>
#include <string>

#include "settings_window.h"
#include "y2skk_settings.h"

int main(int argc, char **argv)
{
    // Handle informational flags before constructing QApplication so they work
    // without a display (headless / no X11 or Wayland session).
    for (int i = 1; i < argc; ++i) {
        const std::string arg = argv[i];
        if (arg == "--version") {
            char *v = y2skk_settings_version();
            std::printf("y2skk-settings-qt6 %s\n", v ? v : "");
            y2skk_settings_string_free(v);
            return 0;
        }
        if (arg == "--help") {
            std::printf(
                "Usage: y2skk-settings-qt6 [OPTIONS]\n"
                "  --version  Print version and exit\n"
                "  --help     Print this help and exit\n"
                "Standard Qt options (e.g. -style) are also accepted.\n");
            return 0;
        }
    }

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
