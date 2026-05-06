#include "swap_ui_plugin.h"
#include "logos_api.h"
#include "swap_api.h"
#include <QDebug>

SwapUiPlugin::SwapUiPlugin(QObject* parent)
    : SwapUiSimpleSource(parent)
{
    setStatus("Initializing");
    setSwapRole(QString{});
    setRunning(false);
    setLastResultJson(QString{});
}

SwapUiPlugin::~SwapUiPlugin()
{
    delete m_swap;
}

void SwapUiPlugin::initLogos(LogosAPI* api)
{
    m_logosAPI = api;
    m_swap = new Swap(api);
    setBackend(this);
    setStatus("Ready");
    qDebug() << "SwapUiPlugin: initialized";
}

void SwapUiPlugin::setRole(const QString& role)
{
    if (role != QStringLiteral("maker") && role != QStringLiteral("taker")) {
        setStatus(QStringLiteral("Invalid role: %1").arg(role));
        return;
    }
    setSwapRole(role);
    setStatus(QStringLiteral("Role: %1").arg(role));
}

void SwapUiPlugin::fetchBalances(const QString& configJson)
{
    if (!m_swap) {
        setStatus("Swap client not ready");
        return;
    }

    setRunning(true);
    setStatus("Fetching balances...");

    m_swap->fetchBalancesAsync(configJson, [this](QString result) {
        setLastResultJson(result);
        setStatus("Balances fetched");
        setRunning(false);
    });
}
