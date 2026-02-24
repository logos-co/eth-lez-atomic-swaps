#ifndef LEZ_ATOMIC_SWAP_MODULE_H
#define LEZ_ATOMIC_SWAP_MODULE_H

#include <QObject>
#include <QQmlEngine>
#include <QThreadPool>

#ifdef LOGOS_CORE_PLUGIN
#include <logos/sdk/plugin_interface.h>
#endif

#include "i_lez_atomic_swap_module.h"

class SwapBackend;

class LezAtomicSwapModule : public QObject
#ifdef LOGOS_CORE_PLUGIN
    , public PluginInterface
#endif
    , public ILezAtomicSwapModule
{
    Q_OBJECT
    Q_INTERFACES(ILezAtomicSwapModule)

public:
    explicit LezAtomicSwapModule(QObject *parent = nullptr);
    ~LezAtomicSwapModule() override;

    // ILezAtomicSwapModule
    void initLogos() override;

    // Register QML types and context properties on the given engine.
    void registerQmlTypes(QQmlEngine *engine);

    SwapBackend *swapBackend() const { return m_swapBackend; }

private:
    SwapBackend *m_swapBackend = nullptr;
    QThreadPool m_threadPool;
};

#endif // LEZ_ATOMIC_SWAP_MODULE_H
