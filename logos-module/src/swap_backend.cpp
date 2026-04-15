#include "swap_backend.h"

#include <QDateTime>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QMetaObject>
#include <QtConcurrent>
#include <cstdlib>
#include <thread>

#ifdef LOGOS_APP_PLUGIN
#include <logos_api.h>
#include <logos_api_client.h>
#include <QPluginLoader>
#include <QStandardPaths>
#include <QDir>
#include <QFile>
#endif

// Helper: call FFI, take ownership of returned string, free it.
static QString ffiToQString(char *raw)
{
    if (!raw)
        return QStringLiteral(R"({"error":"null pointer from FFI"})");
    QString result = QString::fromUtf8(raw);
    swap_ffi_free_string(raw);
    return result;
}

// ---------------------------------------------------------------------------
// Construction / destruction
// ---------------------------------------------------------------------------

SwapBackend::SwapBackend(QThreadPool *pool, QObject *parent)
    : QObject(parent)
    , m_threadPool(pool)
    , m_makerProgressCtx{this, true}
    , m_takerProgressCtx{this, false}
    , m_messagingCtx{this}
{
    connect(&m_makerWatcher, &QFutureWatcher<QString>::finished, this, [this]() {
        setMakerResultJson(m_makerWatcher.result());
        setMakerRunning(false);
        fetchBalances();
    });

    connect(&m_takerWatcher, &QFutureWatcher<QString>::finished, this, [this]() {
        setTakerResultJson(m_takerWatcher.result());
        setTakerRunning(false);
        fetchBalances();
    });

    connect(&m_balanceWatcher, &QFutureWatcher<QString>::finished, this, [this]() {
        auto doc = QJsonDocument::fromJson(m_balanceWatcher.result().toUtf8());
        auto obj = doc.object();
        auto setIfPresent = [&](const QString &key, QString &field, auto signal) {
            if (!obj[key].isNull()) {
                QString val = obj[key].toString();
                if (field != val) { field = val; emit (this->*signal)(); }
            }
        };
        setIfPresent("eth_address", m_ethAddress, &SwapBackend::ethAddressChanged);
        setIfPresent("eth_balance", m_ethBalance, &SwapBackend::ethBalanceChanged);
        setIfPresent("lez_account", m_lezAccount, &SwapBackend::lezAccountChanged);
        setIfPresent("lez_balance", m_lezBalance, &SwapBackend::lezBalanceChanged);
    });

    connect(&m_publishWatcher, &QFutureWatcher<QString>::finished, this, [this]() {
        QString result = m_publishWatcher.result();
        auto doc = QJsonDocument::fromJson(result.toUtf8());
        auto obj = doc.object();
        if (obj.contains("preimage")) {
            m_publishedPreimage = obj["preimage"].toString();
        }
        emit offerPublished(result);
    });

    connect(&m_fetchWatcher, &QFutureWatcher<QString>::finished, this, [this]() {
        emit offersFetched(m_fetchWatcher.result());
    });

    connect(&m_autoAcceptWatcher, &QFutureWatcher<QString>::finished, this, [this]() {
        m_autoAcceptRunning = false;
        emit autoAcceptRunningChanged();
        emit runningChanged();
        fetchBalances();
    });
}

SwapBackend::~SwapBackend()
{
    m_balanceWatcher.waitForFinished();
    m_makerWatcher.waitForFinished();
    m_takerWatcher.waitForFinished();
    m_autoAcceptWatcher.waitForFinished();
    m_publishWatcher.waitForFinished();
    m_fetchWatcher.waitForFinished();
}

// ---------------------------------------------------------------------------
// Config setters (macro to reduce boilerplate)
// ---------------------------------------------------------------------------

#define SETTER(Name, field, signal) \
    void SwapBackend::set##Name(const QString &v) { \
        if (field != v) { field = v; emit signal(); } \
    }

SETTER(EthRpcUrl, m_ethRpcUrl, ethRpcUrlChanged)
SETTER(EthPrivateKey, m_ethPrivateKey, ethPrivateKeyChanged)
SETTER(EthHtlcAddress, m_ethHtlcAddress, ethHtlcAddressChanged)
SETTER(LezSequencerUrl, m_lezSequencerUrl, lezSequencerUrlChanged)
SETTER(LezSigningKey, m_lezSigningKey, lezSigningKeyChanged)
SETTER(LezWalletHome, m_lezWalletHome, lezWalletHomeChanged)
SETTER(LezAccountId, m_lezAccountId, lezAccountIdChanged)
SETTER(LezHtlcProgramId, m_lezHtlcProgramId, lezHtlcProgramIdChanged)
SETTER(LezAmount, m_lezAmount, lezAmountChanged)
SETTER(EthAmount, m_ethAmount, ethAmountChanged)
SETTER(LezTimelockMinutes, m_lezTimelockMinutes, lezTimelockMinutesChanged)
SETTER(EthTimelockMinutes, m_ethTimelockMinutes, ethTimelockMinutesChanged)
SETTER(EthRecipientAddress, m_ethRecipientAddress, ethRecipientAddressChanged)
SETTER(LezTakerAccountId, m_lezTakerAccountId, lezTakerAccountIdChanged)
SETTER(PollIntervalMs, m_pollIntervalMs, pollIntervalMsChanged)

#undef SETTER

// ---------------------------------------------------------------------------
// Maker state helpers
// ---------------------------------------------------------------------------

void SwapBackend::setMakerRunning(bool v)
{
    if (m_makerRunning != v) {
        m_makerRunning = v;
        emit makerRunningChanged();
        emit runningChanged();
    }
}

void SwapBackend::setMakerCurrentStep(const QString &v)
{
    if (m_makerCurrentStep != v) {
        m_makerCurrentStep = v;
        emit makerCurrentStepChanged();
    }
}

void SwapBackend::addMakerProgressStep(const QString &v)
{
    m_makerProgressSteps.append(v);
    emit makerProgressStepsChanged();
}

void SwapBackend::clearMakerProgress()
{
    m_makerProgressSteps.clear();
    emit makerProgressStepsChanged();
    setMakerCurrentStep({});
    setMakerResultJson({});
}

void SwapBackend::setMakerResultJson(const QString &v)
{
    if (m_makerResultJson != v) {
        m_makerResultJson = v;
        emit makerResultJsonChanged();
    }
}

// ---------------------------------------------------------------------------
// Taker state helpers
// ---------------------------------------------------------------------------

void SwapBackend::setTakerRunning(bool v)
{
    if (m_takerRunning != v) {
        m_takerRunning = v;
        emit takerRunningChanged();
        emit runningChanged();
    }
}

void SwapBackend::setTakerCurrentStep(const QString &v)
{
    if (m_takerCurrentStep != v) {
        m_takerCurrentStep = v;
        emit takerCurrentStepChanged();
    }
}

void SwapBackend::addTakerProgressStep(const QString &v)
{
    m_takerProgressSteps.append(v);
    emit takerProgressStepsChanged();
}

void SwapBackend::clearTakerProgress()
{
    m_takerProgressSteps.clear();
    emit takerProgressStepsChanged();
    setTakerCurrentStep({});
    setTakerResultJson({});
}

void SwapBackend::setTakerResultJson(const QString &v)
{
    if (m_takerResultJson != v) {
        m_takerResultJson = v;
        emit takerResultJsonChanged();
    }
}

// ---------------------------------------------------------------------------
// Config JSON
// ---------------------------------------------------------------------------

QByteArray SwapBackend::configJson() const
{
    QJsonObject obj;
    obj["eth_rpc_url"] = m_ethRpcUrl;
    obj["eth_private_key"] = m_ethPrivateKey;
    obj["eth_htlc_address"] = m_ethHtlcAddress;
    obj["lez_sequencer_url"] = m_lezSequencerUrl;
    if (!m_lezSigningKey.isEmpty())
        obj["lez_signing_key"] = m_lezSigningKey;
    if (!m_lezWalletHome.isEmpty())
        obj["lez_wallet_home"] = m_lezWalletHome;
    if (!m_lezAccountId.isEmpty())
        obj["lez_account_id"] = m_lezAccountId;
    obj["lez_htlc_program_id"] = m_lezHtlcProgramId;
    obj["lez_amount"] = m_lezAmount;
    obj["eth_amount"] = m_ethAmount;
    obj["lez_timelock_minutes"] = m_lezTimelockMinutes;
    obj["eth_timelock_minutes"] = m_ethTimelockMinutes;
    obj["eth_recipient_address"] = m_ethRecipientAddress;
    obj["lez_taker_account_id"] = m_lezTakerAccountId;
    obj["poll_interval_ms"] = m_pollIntervalMs;
    return QJsonDocument(obj).toJson(QJsonDocument::Compact);
}

// ---------------------------------------------------------------------------
// Load .env
// ---------------------------------------------------------------------------

void SwapBackend::loadEnv()
{
    auto *result = swap_ffi_load_env(nullptr);
    ffiToQString(result); // just frees the string

    // Read env vars into properties.
    auto env = [](const char *name, const QString &fallback = {}) -> QString {
        const char *val = std::getenv(name);
        return val ? QString::fromUtf8(val) : fallback;
    };

    m_swapRole = env("SWAP_ROLE").toLower();

    setEthRpcUrl(env("ETH_RPC_URL"));
    setEthPrivateKey(env("ETH_PRIVATE_KEY"));
    setEthHtlcAddress(env("ETH_HTLC_ADDRESS"));
    setLezSequencerUrl(env("LEZ_SEQUENCER_URL", "http://localhost:8080"));
    setLezSigningKey(env("LEZ_SIGNING_KEY"));
    setLezWalletHome(env("LEZ_WALLET_HOME"));
    setLezAccountId(env("LEZ_ACCOUNT_ID"));
    setLezHtlcProgramId(env("LEZ_HTLC_PROGRAM_ID"));
    setLezAmount(env("LEZ_AMOUNT", "1"));
    setEthAmount(env("ETH_AMOUNT", "1"));
    setLezTimelockMinutes(env("LEZ_TIMELOCK_MINUTES", "10"));
    setEthTimelockMinutes(env("ETH_TIMELOCK_MINUTES", "5"));
    setEthRecipientAddress(env("ETH_RECIPIENT_ADDRESS"));
    setLezTakerAccountId(env("LEZ_TAKER_ACCOUNT_ID"));
    setPollIntervalMs(env("POLL_INTERVAL_MS", "2000"));

    fetchBalances();
}

// ---------------------------------------------------------------------------
// Load config from JSON object (for Logos Core host injection)
// ---------------------------------------------------------------------------

void SwapBackend::loadConfig(const QJsonObject &config)
{
    auto val = [&config](const QString &key, const QString &fallback = {}) -> QString {
        if (config.contains(key))
            return config[key].toString();
        return fallback;
    };

    m_swapRole = val("swap_role").toLower();

    setEthRpcUrl(val("eth_rpc_url"));
    setEthPrivateKey(val("eth_private_key"));
    setEthHtlcAddress(val("eth_htlc_address"));
    setLezSequencerUrl(val("lez_sequencer_url", "http://localhost:8080"));
    setLezSigningKey(val("lez_signing_key"));
    setLezWalletHome(val("lez_wallet_home"));
    setLezAccountId(val("lez_account_id"));
    setLezHtlcProgramId(val("lez_htlc_program_id"));
    setLezAmount(val("lez_amount", "1"));
    setEthAmount(val("eth_amount", "1"));
    setLezTimelockMinutes(val("lez_timelock_minutes", "10"));
    setEthTimelockMinutes(val("eth_timelock_minutes", "5"));
    setEthRecipientAddress(val("eth_recipient_address"));
    setLezTakerAccountId(val("lez_taker_account_id"));
    setPollIntervalMs(val("poll_interval_ms", "2000"));

    fetchBalances();
}

// ---------------------------------------------------------------------------
// Fetch balances
// ---------------------------------------------------------------------------

void SwapBackend::fetchBalances()
{
    // Skip if a balance fetch is already in flight to avoid flooding the thread pool.
    if (m_balanceWatcher.isRunning())
        return;

    QByteArray cfg = configJson();

    auto future = QtConcurrent::run(m_threadPool, [cfg]() -> QString {
        auto *result = swap_ffi_fetch_balances(cfg.constData());
        return ffiToQString(result);
    });

    m_balanceWatcher.setFuture(future);
}

// ---------------------------------------------------------------------------
// Progress callback (extern "C" to match ProgressCallback typedef)
// ---------------------------------------------------------------------------

extern "C" void progressCallbackTrampoline(const char *json, void *userData)
{
    auto *ctx = static_cast<ProgressContext *>(userData);
    auto *self = ctx->backend;
    bool isMaker = ctx->isMaker;
    QString msg = QString::fromUtf8(json);
    QMetaObject::invokeMethod(self, [self, msg, isMaker]() {
        self->handleProgress(msg, isMaker);
    }, Qt::QueuedConnection);
}

void SwapBackend::handleProgress(const QString &json, bool isMaker)
{
    auto doc = QJsonDocument::fromJson(json.toUtf8());
    auto obj = doc.object();
    QString step = obj["step"].toString();

    // Track tx hashes for history entries
    if (step == "EthClaimed" || step == "LezClaimed") {
        m_lastEthTx = obj["data"].toObject()["tx_hash"].toString();
    }
    if (step == "LezLocked") {
        m_lastLezTx = obj["data"].toObject()["tx_hash"].toString();
    }

    // Handle auto-accept loop events
    if (step == "AutoAcceptIteration") {
        m_autoAcceptIteration = obj["data"].toObject()["iteration"].toInt();
        emit autoAcceptIterationChanged();
        // Reset per-swap progress for new iteration
        clearMakerProgress();
        return;
    }
    if (step == "AutoAcceptSwapCompleted") {
        m_autoAcceptCompleted++;
        emit autoAcceptCompletedChanged();
        QJsonObject entry;
        entry["status"] = QStringLiteral("completed");
        entry["lez_amount"] = m_lezAmount;
        entry["eth_amount"] = m_ethAmount;
        entry["eth_tx"] = m_lastEthTx;
        entry["lez_tx"] = m_lastLezTx;
        entry["timestamp"] = QDateTime::currentMSecsSinceEpoch();
        m_swapHistory.prepend(QString::fromUtf8(
            QJsonDocument(entry).toJson(QJsonDocument::Compact)));
        emit swapHistoryChanged();
        m_lastEthTx.clear();
        m_lastLezTx.clear();
        return;
    }
    if (step == "AutoAcceptSwapFailed") {
        auto data = obj["data"].toObject();
        m_autoAcceptFailed++;
        emit autoAcceptFailedChanged();
        QJsonObject entry;
        entry["status"] = QStringLiteral("failed");
        entry["error"] = data["error"].toString();
        entry["timestamp"] = QDateTime::currentMSecsSinceEpoch();
        m_swapHistory.prepend(QString::fromUtf8(
            QJsonDocument(entry).toJson(QJsonDocument::Compact)));
        emit swapHistoryChanged();
        m_lastEthTx.clear();
        m_lastLezTx.clear();
        return;
    }
    if (step == "AutoAcceptInsufficientFunds") {
        auto data = obj["data"].toObject();
        QJsonObject entry;
        entry["status"] = QStringLiteral("insufficient_funds");
        entry["lez_balance"] = data["lez_balance"].toString();
        entry["lez_required"] = data["lez_required"].toString();
        entry["timestamp"] = QDateTime::currentMSecsSinceEpoch();
        m_swapHistory.prepend(QString::fromUtf8(
            QJsonDocument(entry).toJson(QJsonDocument::Compact)));
        emit swapHistoryChanged();
        return;
    }
    if (step == "AutoAcceptStarted" || step == "AutoAcceptStopped" || step == "AutoAcceptCancelled") {
        return; // handled by watcher finished signal
    }

    if (isMaker) {
        setMakerCurrentStep(step);
        addMakerProgressStep(step);
    } else {
        setTakerCurrentStep(step);
        addTakerProgressStep(step);
    }
}

// ---------------------------------------------------------------------------
// Swap operations
// ---------------------------------------------------------------------------

void SwapBackend::startMaker(const QString &hashlockHex)
{
    if (m_makerRunning || m_autoAcceptRunning)
        return;
    setMakerRunning(true);
    clearMakerProgress();

    QByteArray cfg = configJson();
    QByteArray hl = hashlockHex.toUtf8();

    auto *ctx = &m_makerProgressCtx;
    auto future = QtConcurrent::run(m_threadPool, [cfg, hl, ctx]() -> QString {
        const char *hlPtr = hl.isEmpty() ? nullptr : hl.constData();
        auto *result = swap_ffi_run_maker(
            cfg.constData(),
            hlPtr,
            progressCallbackTrampoline,
            ctx);
        return ffiToQString(result);
    });

    m_makerWatcher.setFuture(future);
}

void SwapBackend::startTaker(const QString &preimageHex)
{
    if (m_takerRunning)
        return;
    setTakerRunning(true);
    clearTakerProgress();

    QByteArray cfg = configJson();
    QByteArray preimage = preimageHex.toUtf8();

    auto *ctx = &m_takerProgressCtx;
    auto future = QtConcurrent::run(m_threadPool, [cfg, preimage, ctx]() -> QString {
        const char *preimagePtr = preimage.isEmpty() ? nullptr : preimage.constData();
        auto *result = swap_ffi_run_taker(
            cfg.constData(),
            preimagePtr,
            progressCallbackTrampoline,
            ctx);
        return ffiToQString(result);
    });

    m_takerWatcher.setFuture(future);
}

// ---------------------------------------------------------------------------
// Messaging send trampoline (extern "C" to match MessagingSendCallback)
// ---------------------------------------------------------------------------

#ifdef LOGOS_APP_PLUGIN

static const QString OFFERS_TOPIC = QStringLiteral("/atomic-swaps/1/offers/json");

extern "C" void messagingSendTrampoline(const char *topic, const char *payload, void *userData)
{
    Q_UNUSED(topic);
    auto *ctx = static_cast<MessagingContext *>(userData);
    auto *plugin = ctx->backend->m_deliveryPlugin;
    if (!plugin)
        return;

    // Fire-and-forget: detach a short-lived thread so the Rust worker thread
    // is never blocked.  The delivery plugin's send() blocks internally.
    QString p = QString::fromUtf8(payload);
    std::thread([plugin, p]() {
        QMetaObject::invokeMethod(plugin, "send",
                                  Q_ARG(QString, OFFERS_TOPIC),
                                  Q_ARG(QString, p));
    }).detach();
}
#endif

// ---------------------------------------------------------------------------
// Auto-accept loop
// ---------------------------------------------------------------------------

void SwapBackend::startAutoAccept()
{
    qDebug() << "[AtomicSwap] startAutoAccept called";
    if (m_autoAcceptRunning || m_makerRunning) {
        qDebug() << "[AtomicSwap] already running, returning";
        return;
    }

    m_autoAcceptRunning = true;
    emit autoAcceptRunningChanged();
    emit runningChanged();

    m_autoAcceptCompleted = 0;
    m_autoAcceptFailed = 0;
    m_autoAcceptIteration = 0;
    m_swapHistory.clear();
    m_lastEthTx.clear();
    m_lastLezTx.clear();
    emit autoAcceptCompletedChanged();
    emit autoAcceptFailedChanged();
    emit autoAcceptIterationChanged();
    emit swapHistoryChanged();

    clearMakerProgress();

    QByteArray cfg = configJson();
    auto *progressCtx = &m_makerProgressCtx;

    qDebug() << "[AtomicSwap] dispatching run_maker_loop to thread pool";

#ifdef LOGOS_APP_PLUGIN
    auto *msgCtx = m_deliveryPlugin ? &m_messagingCtx : nullptr;
    qDebug() << "[AtomicSwap] delivery plugin:" << (m_deliveryPlugin ? "yes" : "no");
    auto future = QtConcurrent::run(m_threadPool, [cfg, progressCtx, msgCtx]() -> QString {
        qDebug() << "[AtomicSwap] thread pool: calling swap_ffi_run_maker_loop";
        auto *result = swap_ffi_run_maker_loop(
            cfg.constData(),
            progressCallbackTrampoline,
            progressCtx,
            msgCtx ? messagingSendTrampoline : nullptr,
            msgCtx);
        return ffiToQString(result);
    });
#else
    auto future = QtConcurrent::run(m_threadPool, [cfg, progressCtx]() -> QString {
        auto *result = swap_ffi_run_maker_loop(
            cfg.constData(),
            progressCallbackTrampoline,
            progressCtx,
            nullptr,
            nullptr);
        return ffiToQString(result);
    });
#endif

    m_autoAcceptWatcher.setFuture(future);
}

void SwapBackend::stopAutoAccept()
{
    swap_ffi_stop_maker_loop();
}

// ---------------------------------------------------------------------------
// Delivery module (LogosAPI) — only available in logos-app plugin mode
// ---------------------------------------------------------------------------

#ifdef LOGOS_APP_PLUGIN

void SwapBackend::setLogosAPI(LogosAPI *api)
{
    m_logosAPI = api;

    // Load delivery module directly (bypasses framework RPC which can't load it).
    if (!m_deliveryPlugin) {
        initDeliveryNode();
    }

    emit deliveryAvailableChanged();
}

void SwapBackend::initDeliveryNode()
{
    // Resolve the delivery module dylib from the standard modules directory.
    // Non-portable (Nix) builds append "Nix" to AppDataLocation.
    QString appData = QStandardPaths::writableLocation(QStandardPaths::AppDataLocation);
    QString pluginPath = appData + QStringLiteral("Nix/modules/delivery_module/delivery_module_plugin.dylib");
    if (!QFile::exists(pluginPath)) {
        // Fallback: portable build (no "Nix" suffix).
        pluginPath = appData + QStringLiteral("/modules/delivery_module/delivery_module_plugin.dylib");
    }

    qDebug() << "[AtomicSwap] loading delivery module from:" << pluginPath;

    auto *loader = new QPluginLoader(pluginPath, this);
    if (!loader->load()) {
        qWarning() << "[AtomicSwap] failed to load delivery module:" << loader->errorString();
        delete loader;
        return;
    }

    m_deliveryPlugin = loader->instance();
    if (!m_deliveryPlugin) {
        qWarning() << "[AtomicSwap] delivery module instance is null";
        return;
    }

    qDebug() << "[AtomicSwap] delivery module loaded:" << m_deliveryPlugin->metaObject()->className();

    // Run createNode + start + subscribe on thread pool (they block internally).
    bool isMaker = (m_swapRole != QStringLiteral("taker"));
    auto *plugin = m_deliveryPlugin;
    QtConcurrent::run(m_threadPool, [this, plugin, isMaker]() {
        uint16_t p2pPort = isMaker ? 60000 : 60001;
        uint16_t discv5Port = isMaker ? 9000 : 9001;

        QJsonObject networkingConfig;
        networkingConfig["listenIpv4"] = QStringLiteral("0.0.0.0");
        networkingConfig["p2pTcpPort"] = p2pPort;
        networkingConfig["discv5UdpPort"] = discv5Port;

        QJsonObject autoSharding;
        autoSharding["numShardsInCluster"] = 1;

        QJsonObject messageValidation;
        messageValidation["maxMessageSize"] = QStringLiteral("150 KiB");

        QJsonObject protocolsConfig;
        protocolsConfig["entryNodes"] = QJsonArray({
            QStringLiteral("enrtree://AIRVQ5DDA4FFWLRBCHJWUWOO6X6S4ZTZ5B667LQ6AJU6PEYDLRD5O@sandbox.waku.nodes.status.im")
        });
        protocolsConfig["clusterId"] = 1;
        protocolsConfig["autoShardingConfig"] = autoSharding;
        protocolsConfig["messageValidation"] = messageValidation;

        QJsonObject nodeConfig;
        nodeConfig["mode"] = QStringLiteral("Core");
        nodeConfig["networkingConfig"] = networkingConfig;
        nodeConfig["protocolsConfig"] = protocolsConfig;
        nodeConfig["logLevel"] = QStringLiteral("DEBUG");

        QString cfgJson = QString::fromUtf8(
            QJsonDocument(nodeConfig).toJson(QJsonDocument::Compact));

        qDebug() << "[AtomicSwap] delivery createNode config:" << cfgJson;

        bool createOk = false;
        QMetaObject::invokeMethod(plugin, "createNode", Qt::DirectConnection,
                                  Q_RETURN_ARG(bool, createOk),
                                  Q_ARG(QString, cfgJson));
        qDebug() << "[AtomicSwap] delivery createNode:" << createOk;
        if (!createOk) {
            qWarning() << "[AtomicSwap] delivery createNode failed";
            return;
        }

        bool startOk = false;
        QMetaObject::invokeMethod(plugin, "start", Qt::DirectConnection,
                                  Q_RETURN_ARG(bool, startOk));
        qDebug() << "[AtomicSwap] delivery start:" << startOk;
        if (!startOk) {
            qWarning() << "[AtomicSwap] delivery start failed";
            return;
        }

        bool subOk = false;
        QMetaObject::invokeMethod(plugin, "subscribe", Qt::DirectConnection,
                                  Q_RETURN_ARG(bool, subOk),
                                  Q_ARG(QString, OFFERS_TOPIC));
        qDebug() << "[AtomicSwap] delivery subscribe:" << subOk;

        QMetaObject::invokeMethod(this, [this]() {
            m_deliveryNodeStarted = true;
            emit deliveryAvailableChanged();
            qDebug() << "[AtomicSwap] delivery node started and subscribed to" << OFFERS_TOPIC;
        }, Qt::QueuedConnection);
    });
}

void SwapBackend::publishOffer()
{
    if (!m_deliveryPlugin) {
        emit offerPublished(QStringLiteral(R"({"error":"delivery module not available"})"));
        return;
    }

    QJsonObject offer;
    offer["hashlock"] = QString();
    offer["lez_amount"] = m_lezAmount;
    offer["eth_amount"] = m_ethAmount;
    offer["maker_eth_address"] = m_ethRecipientAddress;
    offer["maker_lez_account"] = m_lezAccount;
    offer["lez_timelock"] = m_lezTimelockMinutes;
    offer["eth_timelock"] = m_ethTimelockMinutes;
    offer["lez_htlc_program_id"] = m_lezHtlcProgramId;
    offer["eth_htlc_address"] = m_ethHtlcAddress;

    QString payload = QString::fromUtf8(
        QJsonDocument(offer).toJson(QJsonDocument::Compact));

    auto *plugin = m_deliveryPlugin;
    auto future = QtConcurrent::run(m_threadPool, [plugin, payload]() -> QString {
        QMetaObject::invokeMethod(plugin, "send", Qt::DirectConnection,
                                  Q_ARG(QString, OFFERS_TOPIC),
                                  Q_ARG(QString, payload));
        QJsonObject obj;
        obj["ok"] = true;
        return QString::fromUtf8(QJsonDocument(obj).toJson(QJsonDocument::Compact));
    });

    m_publishWatcher.setFuture(future);
}

void SwapBackend::fetchOffers()
{
    if (!m_deliveryPlugin) {
        emit offersFetched(QStringLiteral(R"({"offers":[],"error":"delivery module not available"})"));
        return;
    }

    auto *plugin = m_deliveryPlugin;
    auto future = QtConcurrent::run(m_threadPool, [plugin]() -> QString {
        bool subOk = false;
        QMetaObject::invokeMethod(plugin, "subscribe", Qt::DirectConnection,
                                  Q_RETURN_ARG(bool, subOk),
                                  Q_ARG(QString, OFFERS_TOPIC));
        qDebug() << "[AtomicSwap] fetchOffers subscribe:" << subOk;
        // Offers arrive via event callback — return empty for now.
        return QStringLiteral(R"({"offers":[]})");
    });

    m_fetchWatcher.setFuture(future);
}

#else // !LOGOS_APP_PLUGIN

void SwapBackend::setLogosAPI(LogosAPI *) {}
void SwapBackend::publishOffer() {}
void SwapBackend::fetchOffers()
{
    emit offersFetched(QStringLiteral(R"({"offers":[]})"));
}

#endif // LOGOS_APP_PLUGIN

void SwapBackend::refundLez(const QString &hashlockHex)
{
    if (m_makerRunning)
        return;
    setMakerRunning(true);
    clearMakerProgress();

    QByteArray cfg = configJson();
    QByteArray hl = hashlockHex.toUtf8();

    auto future = QtConcurrent::run(m_threadPool, [cfg, hl]() -> QString {
        auto *result = swap_ffi_refund_lez(cfg.constData(), hl.constData());
        return ffiToQString(result);
    });

    m_makerWatcher.setFuture(future);
}

void SwapBackend::refundEth(const QString &swapIdHex)
{
    if (m_takerRunning)
        return;
    setTakerRunning(true);
    clearTakerProgress();

    QByteArray cfg = configJson();
    QByteArray sid = swapIdHex.toUtf8();

    auto future = QtConcurrent::run(m_threadPool, [cfg, sid]() -> QString {
        auto *result = swap_ffi_refund_eth(cfg.constData(), sid.constData());
        return ffiToQString(result);
    });

    m_takerWatcher.setFuture(future);
}
