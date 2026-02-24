#ifdef LOGOS_CORE_PLUGIN

#include <QObject>
#include <QtPlugin>

#include <logos/sdk/plugin_interface.h>
#include "lez_atomic_swap_module.h"

class LezAtomicSwapPlugin : public QObject, public PluginInterface {
    Q_OBJECT
    Q_PLUGIN_METADATA(IID PluginInterface_iid FILE "../metadata.json")
    Q_INTERFACES(PluginInterface)

public:
    QObject *create(const QString &key) override
    {
        if (key == ILezAtomicSwapModule_iid)
            return new LezAtomicSwapModule();
        return nullptr;
    }
};

#include "plugin.moc"

#endif // LOGOS_CORE_PLUGIN
