#include "swap_backend.h"

#include <QJsonDocument>
#include <QJsonObject>
#include <QMetaObject>
#include <QtConcurrent>
#include <cstdlib>

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
}

SwapBackend::~SwapBackend()
{
    m_balanceWatcher.waitForFinished();
    m_makerWatcher.waitForFinished();
    m_takerWatcher.waitForFinished();
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
SETTER(NwakuUrl, m_nwakuUrl, nwakuUrlChanged)

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
    obj["nwaku_url"] = m_nwakuUrl;
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
    setNwakuUrl(env("NWAKU_URL"));

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
    setNwakuUrl(val("nwaku_url"));

    fetchBalances();
}

// ---------------------------------------------------------------------------
// Fetch balances
// ---------------------------------------------------------------------------

void SwapBackend::fetchBalances()
{
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
    if (m_makerRunning)
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
// Messaging (nwaku REST)
// ---------------------------------------------------------------------------

void SwapBackend::publishOffer()
{
    if (m_nwakuUrl.isEmpty())
        return;

    QByteArray cfg = configJson();
    QByteArray url = m_nwakuUrl.toUtf8();

    auto future = QtConcurrent::run(m_threadPool, [cfg, url]() -> QString {
        auto *result = swap_ffi_publish_offer(cfg.constData(), url.constData());
        return ffiToQString(result);
    });

    m_publishWatcher.setFuture(future);
}

void SwapBackend::fetchOffers()
{
    if (m_nwakuUrl.isEmpty())
        return;

    QByteArray url = m_nwakuUrl.toUtf8();

    auto future = QtConcurrent::run(m_threadPool, [url]() -> QString {
        auto *result = swap_ffi_fetch_offers(url.constData());
        return ffiToQString(result);
    });

    m_fetchWatcher.setFuture(future);
}

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
