#include "swapbackend.h"

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

SwapBackend::SwapBackend(QObject *parent)
    : QObject(parent)
{
    connect(&m_watcher, &QFutureWatcher<QString>::finished, this, [this]() {
        setResultJson(m_watcher.result());
        setRunning(false);
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
    m_watcher.waitForFinished();
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
    obj["lez_signing_key"] = m_lezSigningKey;
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
    setLezHtlcProgramId(env("LEZ_HTLC_PROGRAM_ID"));
    setLezAmount(env("LEZ_AMOUNT", "1000"));
    setEthAmount(env("ETH_AMOUNT", "1000000000000000"));
    setLezTimelockMinutes(env("LEZ_TIMELOCK_MINUTES", "10"));
    setEthTimelockMinutes(env("ETH_TIMELOCK_MINUTES", "5"));
    setEthRecipientAddress(env("ETH_RECIPIENT_ADDRESS"));
    setLezTakerAccountId(env("LEZ_TAKER_ACCOUNT_ID"));
    setPollIntervalMs(env("POLL_INTERVAL_MS", "2000"));
    setNwakuUrl(env("NWAKU_URL"));
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
    setCurrentStep(step);
    addProgressStep(step);
}

// ---------------------------------------------------------------------------
// Swap operations
// ---------------------------------------------------------------------------

void SwapBackend::startMaker(const QString &hashlockHex)
{
    if (m_running)
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
