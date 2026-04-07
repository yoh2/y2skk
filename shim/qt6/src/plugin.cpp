/*
 * plugin.cpp — QPlatformInputContextPlugin for y2skk.
 *
 * Qt loads this plugin when QT_IM_MODULE=y2skk.  The plugin factory creates a
 * Y2skkInputContext which handles all IM communication via the Rust D-Bus layer.
 */

#include <QtGui/qpa/qplatforminputcontextplugin_p.h>
#include "context.h"

class Y2skkInputContextPlugin : public QPlatformInputContextPlugin
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID "org.qt-project.Qt.QPlatformInputContextFactoryInterface.5.1" FILE "y2skk.json")

public:
    QPlatformInputContext *create(const QString &key, const QStringList &) override
    {
        if (key.compare(QLatin1String("y2skk"), Qt::CaseInsensitive) == 0)
            return new Y2skkInputContext;
        return nullptr;
    }
};

#include "plugin.moc"
