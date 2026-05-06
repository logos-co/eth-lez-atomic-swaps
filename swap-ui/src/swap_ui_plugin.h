#ifndef SWAP_UI_PLUGIN_H
#define SWAP_UI_PLUGIN_H

#include <QString>
#include <QVariantList>
#include "swap_ui_interface.h"
#include "LogosViewPluginBase.h"
#include "rep_swap_ui_source.h"

class LogosAPI;
class Swap;

// Three base classes:
//   - SwapUiSimpleSource     : generated from swap_ui.rep; property storage + slot decls.
//   - SwapUiInterface        : extends PluginInterface for Qt plugin loading.
//   - SwapUiViewPluginBase   : provides setBackend() to wire up Qt Remote Objects.
class SwapUiPlugin : public SwapUiSimpleSource,
                     public SwapUiInterface,
                     public SwapUiViewPluginBase
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID SwapUiInterface_iid FILE "metadata.json")
    Q_INTERFACES(SwapUiInterface)

public:
    explicit SwapUiPlugin(QObject* parent = nullptr);
    ~SwapUiPlugin() override;

    QString name()    const override { return "swap_ui"; }
    QString version() const override { return "0.1.0"; }

    Q_INVOKABLE void initLogos(LogosAPI* api);

    // Slots from swap_ui.rep
    void setRole(const QString& role) override;
    void fetchBalances(const QString& configJson) override;

signals:
    void eventResponse(const QString& eventName, const QVariantList& args);

private:
    LogosAPI* m_logosAPI = nullptr;
    Swap* m_swap = nullptr;
};

#endif // SWAP_UI_PLUGIN_H
