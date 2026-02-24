#include "lez_atomic_swap_module.h"
#include "swap_backend.h"

#include <QQmlContext>

LezAtomicSwapModule::LezAtomicSwapModule(QObject *parent)
    : QObject(parent)
{
    // Alloy/tungstenite WebSocket+TLS handshake needs deep stack.
    // Default QtConcurrent pool threads get ~512K which overflows.
    m_threadPool.setMaxThreadCount(4);
    m_threadPool.setStackSize(8 * 1024 * 1024);
}

LezAtomicSwapModule::~LezAtomicSwapModule() = default;

void LezAtomicSwapModule::initLogos()
{
    m_swapBackend = new SwapBackend(&m_threadPool, this);
    m_swapBackend->loadEnv();
}

void LezAtomicSwapModule::registerQmlTypes(QQmlEngine *engine)
{
    if (!m_swapBackend)
        initLogos();

    engine->rootContext()->setContextProperty("swapBackend", m_swapBackend);
}
