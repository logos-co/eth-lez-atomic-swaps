#include "swapbackend.h"

#include <QDateTime>
#include <QJsonDocument>
#include <QJsonObject>
#include <QMetaObject>
#include <QtConcurrent>
#include <QDebug>
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

SwapBackend::SwapBackend(QObject *parent)
    : QObject(parent)
{
    connect(&m_watcher, &QFutureWatcher<QString>::finished, this, [this]() {
        setResultJson(m_watcher.result());
        setRunning(false);
        fetchBalances();
    });

    connect(&m_balanceWatcher, &QFutureWatcher<QString>::finished, this, [this]() {
        qDebug() << "FFI balance result:" << m_balanceWatcher.result();
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
    m_watcher.waitForFinished();
    m_balanceWatcher.waitForFinished();
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
SETTER(NwakuUrl, m_nwakuUrl, nwakuUrlChanged)

#undef SETTER

// ---------------------------------------------------------------------------
// State helpers
// ---------------------------------------------------------------------------

void SwapBackend::setRunning(bool v)
{
    if (m_running != v) {
        m_running = v;
        emit runningChanged();
    }
}

void SwapBackend::setCurrentStep(const QString &v)
{
    if (m_currentStep != v) {
        m_currentStep = v;
        emit currentStepChanged();
    }
}

void SwapBackend::addProgressStep(const QString &v)
{
    m_progressSteps.append(v);
    emit progressStepsChanged();
}

void SwapBackend::clearProgress()
{
    m_progressSteps.clear();
    emit progressStepsChanged();
    setCurrentStep({});
    setResultJson({});
}

void SwapBackend::setResultJson(const QString &v)
{
    if (m_resultJson != v) {
        m_resultJson = v;
        emit resultJsonChanged();
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
    if (!m_nwakuUrl.isEmpty())
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
    setPollIntervalMs(env("POLL_INTERVAL_MS", "500"));
    setNwakuUrl(env("NWAKU_URL"));

    fetchBalances();
}

// ---------------------------------------------------------------------------
// Fetch balances
// ---------------------------------------------------------------------------

void SwapBackend::fetchBalances()
{
    QByteArray cfg = configJson();
    qDebug() << "fetchBalances config:" << cfg;

    auto future = QtConcurrent::run([cfg]() -> QString {
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
    auto *self = static_cast<SwapBackend *>(userData);
    QString msg = QString::fromUtf8(json);
    QMetaObject::invokeMethod(self, [self, msg]() {
        self->handleProgress(msg);
    }, Qt::QueuedConnection);
}

void SwapBackend::handleProgress(const QString &json)
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
        clearProgress();
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
        return;
    }

    setCurrentStep(step);
    addProgressStep(step);
}

// ---------------------------------------------------------------------------
// Swap operations
// ---------------------------------------------------------------------------

void SwapBackend::startMaker(const QString &hashlockHex)
{
    if (m_running || m_autoAcceptRunning)
        return;
    setRunning(true);
    clearProgress();

    QByteArray cfg = configJson();
    QByteArray hl = hashlockHex.toUtf8();

    auto future = QtConcurrent::run([cfg, hl, this]() -> QString {
        const char *hlPtr = hl.isEmpty() ? nullptr : hl.constData();
        auto *result = swap_ffi_run_maker(
            cfg.constData(),
            hlPtr,
            progressCallbackTrampoline,
            this);
        return ffiToQString(result);
    });

    m_watcher.setFuture(future);
}

void SwapBackend::startTaker(const QString &preimageHex)
{
    if (m_running)
        return;
    setRunning(true);
    clearProgress();

    QByteArray cfg = configJson();
    QByteArray preimage = preimageHex.toUtf8();

    auto future = QtConcurrent::run([cfg, preimage, this]() -> QString {
        const char *preimagePtr = preimage.isEmpty() ? nullptr : preimage.constData();
        auto *result = swap_ffi_run_taker(
            cfg.constData(),
            preimagePtr,
            progressCallbackTrampoline,
            this);
        return ffiToQString(result);
    });

    m_watcher.setFuture(future);
}

// ---------------------------------------------------------------------------
// Auto-accept loop
// ---------------------------------------------------------------------------

void SwapBackend::startAutoAccept()
{
    if (m_autoAcceptRunning || m_running)
        return;

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

    clearProgress();

    QByteArray cfg = configJson();

    auto future = QtConcurrent::run([cfg, this]() -> QString {
        auto *result = swap_ffi_run_maker_loop(
            cfg.constData(),
            progressCallbackTrampoline,
            this);
        return ffiToQString(result);
    });

    m_autoAcceptWatcher.setFuture(future);
}

void SwapBackend::stopAutoAccept()
{
    swap_ffi_stop_maker_loop();
}

void SwapBackend::publishOffer()
{
    if (m_nwakuUrl.isEmpty())
        return;

    QByteArray cfg = configJson();
    QByteArray nwaku = m_nwakuUrl.toUtf8();

    auto future = QtConcurrent::run([cfg, nwaku]() -> QString {
        auto *result = swap_ffi_publish_offer(cfg.constData(), nwaku.constData());
        return ffiToQString(result);
    });

    m_publishWatcher.setFuture(future);
}

void SwapBackend::fetchOffers()
{
    if (m_nwakuUrl.isEmpty())
        return;

    QByteArray nwaku = m_nwakuUrl.toUtf8();

    auto future = QtConcurrent::run([nwaku]() -> QString {
        auto *result = swap_ffi_fetch_offers(nwaku.constData());
        return ffiToQString(result);
    });

    m_fetchWatcher.setFuture(future);
}

void SwapBackend::refundLez(const QString &hashlockHex)
{
    if (m_running)
        return;
    setRunning(true);
    clearProgress();

    QByteArray cfg = configJson();
    QByteArray hl = hashlockHex.toUtf8();

    auto future = QtConcurrent::run([cfg, hl]() -> QString {
        auto *result = swap_ffi_refund_lez(cfg.constData(), hl.constData());
        return ffiToQString(result);
    });

    m_watcher.setFuture(future);
}

void SwapBackend::refundEth(const QString &swapIdHex)
{
    if (m_running)
        return;
    setRunning(true);
    clearProgress();

    QByteArray cfg = configJson();
    QByteArray sid = swapIdHex.toUtf8();

    auto future = QtConcurrent::run([cfg, sid]() -> QString {
        auto *result = swap_ffi_refund_eth(cfg.constData(), sid.constData());
        return ffiToQString(result);
    });

    m_watcher.setFuture(future);
}
