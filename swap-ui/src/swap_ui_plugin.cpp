#include "swap_ui_plugin.h"

#include "logos_api.h"
#include "logos_api_client.h"
#include "offer_venue.h"
#include "swap_api.h"
#include "timelock_math.h"

#include <QCoreApplication>
#include <QDateTime>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonValue>
#include <QMetaObject>
#include <QMetaType>
#include <QRandomGenerator>
#include <QRegularExpression>
#include <QSocketNotifier>
#include <QStandardPaths>
#include <QVariant>
#include <QDebug>

#include <cstdio>
#include <fcntl.h>
#include <signal.h>
#include <sys/socket.h>
#include <unistd.h>

namespace {

constexpr const char* kSwapModuleName = "swap";

// Receipt journal caps. The surfaced QML property carries at most this many
// receipts (newest first); the on-disk JSONL keeps everything. One line per
// swap keeps growth modest — rotation is deliberately future work.
constexpr int kMaxSurfacedReceipts = 200;

// Self-pipe (socketpair) for async-signal-safe SIGTERM/SIGINT handling. The
// raw handler (unixSignalHandler, below) may only call async-signal-safe
// functions, so it does nothing but write() one byte here; the actual
// (Qt-based) flush happens on the main thread via the QSocketNotifier set up
// in SwapUiPlugin::installSignalHandlers(). File-scope because a signal
// handler must be a plain function, and at most one SwapUiPlugin instance
// lives per ui-host process. The previous disposition for each signal is
// captured so it can be restored/re-delivered after our flush, so we don't
// swallow ui-host's own shutdown handling.
int g_signalSelfPipe[2] = { -1, -1 };
struct sigaction g_prevSigTerm {};
struct sigaction g_prevSigInt {};

// Out-of-band trace logger. Basecamp installs a Qt message handler that
// swallows qInfo/qWarning/qCritical, so plugin-side diagnostics never reach
// basecamp.log. We bypass Qt entirely by appending to a per-instance file
// under XDG_RUNTIME_DIR (already isolated to /tmp/lbc-{maker,taker} by
// scripts/basecamp-instance.sh) and to stderr with explicit fflush.
void swapUiTrace(const QString& msg)
{
    const QString runtimeDir = qEnvironmentVariable("XDG_RUNTIME_DIR");
    const QString path = !runtimeDir.isEmpty()
        ? QDir(runtimeDir).filePath(QStringLiteral("swap-ui.trace.log"))
        : QDir::temp().filePath(QStringLiteral("swap-ui.trace.log"));

    const QByteArray line =
        QDateTime::currentDateTimeUtc().toString(Qt::ISODateWithMs).toUtf8()
        + " [swap_ui] " + msg.toUtf8() + "\n";

    QFile f(path);
    if (f.open(QIODevice::WriteOnly | QIODevice::Append | QIODevice::Text)) {
        f.write(line);
        f.flush();
    }

    std::fwrite(line.constData(), 1, static_cast<size_t>(line.size()), stderr);
    std::fflush(stderr);
}

QString valueString(const QJsonObject& obj, const QString& key)
{
    const auto value = obj.value(key);
    if (value.isString()) {
        return value.toString();
    }
    if (value.isDouble()) {
        return QString::number(value.toDouble(), 'f', 0);
    }
    if (value.isBool()) {
        return value.toBool() ? QStringLiteral("true") : QStringLiteral("false");
    }
    return {};
}

QString resolveLocalEnvPath(const QString& path)
{
    const QFileInfo direct(path);
    if (direct.exists()) {
        return direct.absoluteFilePath();
    }

    const QString fileName = direct.fileName();
    QDir dir(QDir::currentPath());
    for (int i = 0; i < 6; ++i) {
        const QString relativeCandidate = dir.absoluteFilePath(path);
        if (QFileInfo::exists(relativeCandidate)) {
            return QFileInfo(relativeCandidate).absoluteFilePath();
        }

        if (!fileName.isEmpty()) {
            const QString nameCandidate = dir.absoluteFilePath(fileName);
            if (QFileInfo::exists(nameCandidate)) {
                return QFileInfo(nameCandidate).absoluteFilePath();
            }
        }

        if (!dir.cdUp()) {
            break;
        }
    }

    return path;
}

QString weiToEthValue(QString wei)
{
    wei = wei.trimmed();
    if (wei.isEmpty()) {
        return QStringLiteral("0");
    }
    while (wei.length() > 1 && wei.startsWith(QLatin1Char('0'))) {
        wei.remove(0, 1);
    }
    if (wei.length() <= 18) {
        QString fraction = wei.rightJustified(18, QLatin1Char('0'));
        while (fraction.endsWith(QLatin1Char('0'))) {
            fraction.chop(1);
        }
        return fraction.isEmpty() ? QStringLiteral("0") : QStringLiteral("0.%1").arg(fraction);
    }

    QString whole = wei.left(wei.length() - 18);
    QString fraction = wei.right(18);
    while (fraction.endsWith(QLatin1Char('0'))) {
        fraction.chop(1);
    }
    return fraction.isEmpty() ? whole : QStringLiteral("%1.%2").arg(whole, fraction);
}

QString compactVariantJson(const QVariant& value)
{
    if (!value.isValid()) {
        return QStringLiteral("{}");
    }
    if (value.typeId() == QMetaType::QString) {
        return value.toString();
    }
    const QJsonDocument doc = QJsonDocument::fromVariant(value);
    return QString::fromUtf8(doc.toJson(QJsonDocument::Compact));
}

// Default LEZ sequencer URL pre-filled into the Config tab. Targets the public
// testnet so a stock catalog install works out of the box; the field stays
// user-editable. For local dev (localnet runs on :3040) set the env override
// rather than baking a localhost URL here.
QString defaultLezSequencerUrl()
{
    const QString override = qEnvironmentVariable("SWAP_UI_LEZ_SEQUENCER_URL");
    return override.isEmpty() ? QStringLiteral("https://testnet.lez.logos.co") : override;
}

// Default ETH RPC endpoint pre-filled into the Config tab: the same public
// Sepolia WSS endpoint lez-mcp bakes in (lez-mcp/src/config.rs
// DEFAULT_ETH_RPC_URL). Previously this field started empty, so a stock
// install couldn't even validate its config without the user finding and
// pasting an RPC URL first. Basecamp requires a WebSocket endpoint; an env
// override is honored for local dev (point at anvil's ws:// port).
QString defaultEthRpcUrl()
{
    const QString override = qEnvironmentVariable("SWAP_UI_ETH_RPC_URL");
    return override.isEmpty()
        ? QStringLiteral("wss://ethereum-sepolia-rpc.publicnode.com")
        : override;
}

// Default ETH HTLC contract address pre-filled into the Config tab: the
// canonical shared deployment on Sepolia (chainId 11155111, deployed
// 2026-07-21, tx 0xb634a97c…e7c1, minTimelockDelta=300). Updating this single
// constant ships a new default to every install. Takers still inherit the
// address from accepted offers. An env override is honored for dev/testing.
QString defaultEthHtlcAddress()
{
    const QString override = qEnvironmentVariable("SWAP_UI_ETH_HTLC_ADDRESS");
    return override.isEmpty()
        ? QStringLiteral("0x351B0EA07739FA9F6769213927D7836a790A5FAF")
        : override;
}

} // namespace

// Reset every config PROP back to its built-in default. Shared by the
// constructor (fresh in-memory state) and resetConfig() (the "Reset app
// data" control) so the two can never drift apart.
void SwapUiPlugin::applyDefaultConfig()
{
    setEthRpcUrl(defaultEthRpcUrl());
    setEthPrivateKey(QString{});
    setEthHtlcAddress(defaultEthHtlcAddress());
    setLezSequencerUrl(defaultLezSequencerUrl());
    setLezSigningKey(QString{});
    setLezWalletHome(QString{});
    setLezAccountId(QString{});
    setLezHtlcProgramId(QString{});
    // 150 LEZ: exactly one pinata faucet claim, so a fresh testnet account can
    // actually fund a default offer. 1 (the old default) is a rounding error
    // next to the 20/40-minute standing-bot timelocks and not worth adjusting
    // for on its own, but paired with 1 ETH below it made the stock offer
    // impossible to fill on Sepolia.
    setLezAmount(QStringLiteral("150"));
    // 0.0002 ETH, not 1: 1 whole ETH is unfillable for essentially every
    // Sepolia faucet-funded tester, so the default offer was dead on arrival.
    setEthAmount(QStringLiteral("0.0002"));
    // LEZ 15 / ETH 25: validateConfig only requires eth > lez, but the
    // maker-loop runtime gate needs eth >= lez + margin (5) + transit slack
    // (2) = lez + 7 (see bot::validate_timelocks). 15/25 clears that with
    // margin to spare (25 >= 22), unlike the old 5/15 pair, which sat exactly
    // on the boundary and rejected every taker lock by the tx-transit delta.
    setLezTimelockMinutes(QStringLiteral("15"));
    setEthTimelockMinutes(QStringLiteral("25"));
    setEthRecipientAddress(QString{});
    setLezTakerAccountId(QString{});
    setPollIntervalMs(QStringLiteral("2000"));
}

SwapUiPlugin::SwapUiPlugin(QObject* parent)
    : SwapUiSimpleSource(parent)
{
    m_deliveryPortsShift = 100 + static_cast<int>(QRandomGenerator::global()->bounded(4500));
    setStatus(QStringLiteral("Initializing"));
    setErrorMessage(QString{});
    setSwapRole(QString{});
    setRunning(false);
    setLastResultJson(QString{});
    setValidationErrorsJson(QStringLiteral("{}"));

    applyDefaultConfig();

    setEthAddress(QString{});
    setEthBalance(QString{});
    setLezAccount(QString{});
    setLezBalance(QString{});

    setMakerRunning(false);
    setMakerJobId(QString{});
    setMakerCurrentStep(QString{});
    setMakerProgressSteps(QStringList{});
    setMakerResultJson(QString{});

    setTakerRunning(false);
    setTakerJobId(QString{});
    setTakerCurrentStep(QString{});
    setTakerProgressSteps(QStringList{});
    setTakerResultJson(QString{});

    setAutoAcceptRunning(false);
    setAutoAcceptJobId(QString{});
    setAutoAcceptCompleted(0);
    setAutoAcceptFailed(0);
    setAutoAcceptIteration(0);
    setSwapHistory(QStringList{});

    setMessagingConnected(false);
    setMessagingPeerCount(0);
    setMessagingPeerCountKnown(false);
    setMessagingConnectionStatus(QString{});
    setMessagingHint(QString{});
    setMessagingRetrying(false);
    setOffersJson(QString{});
    setOfferResultJson(QString{});
    setBalancesLoading(false);
    setMessagingLoading(false);
    setOffersLoading(false);
    setPublishingLoading(false);
    setRefundsLoading(false);

    setCoordinationActiveHashlock(QString{});
    setCoordinationEventsJson(QStringLiteral("[]"));
    setCoordinationLastResultJson(QString{});

    setReceiptsJson(QStringLiteral("[]"));

    setSetupRunning(false);
    setSetupJobId(QString{});
    setSetupStep(QString{});
    setSetupError(QString{});
    setSetupBalance(QString{});
    setSetupTarget(QString{});
    setSetupClaims(0);

    m_messagingPollTimer.setInterval(2000);
    connect(&m_messagingPollTimer, &QTimer::timeout,
            this, &SwapUiPlugin::pollMessagingStatus);

    m_coordinationPollTimer.setInterval(1000);
    m_coordinationPollTimer.setSingleShot(false);
    connect(&m_coordinationPollTimer, &QTimer::timeout,
            this, &SwapUiPlugin::coordinationPollSwapEvents);

    // Config-file save is now synchronous (see scheduleConfigSave()) — no
    // debounce timer to wire up here any more. installSignalHandlers() below
    // covers the out-of-process teardown case where even a synchronous write
    // moments ago might be followed by more edits before a graceful quit.
    installSignalHandlers();
    if (auto* app = QCoreApplication::instance()) {
        // Belt-and-braces: if the host ever DOES route Quit through a
        // graceful QCoreApplication shutdown (rather than the raw
        // termination signal this was written to withstand), flush there
        // too. saveConfigToDisk() is idempotent/cheap so this costs nothing
        // when the synchronous save already covers it.
        connect(app, &QCoreApplication::aboutToQuit,
                this, &SwapUiPlugin::saveConfigToDisk);
    }

    validateConfig();
}

SwapUiPlugin::~SwapUiPlugin()
{
    m_messagingPollTimer.stop();
    m_coordinationPollTimer.stop();
    // Config saves are synchronous now (see scheduleConfigSave()), so there
    // is no pending debounced write to flush here — this is purely a
    // best-effort net in case that ever regresses.
    saveConfigToDisk();
    if (m_swap) {
        if (!makerJobId().isEmpty()) {
            m_swap->stopJob(makerJobId());
        }
        if (!takerJobId().isEmpty()) {
            m_swap->stopJob(takerJobId());
        }
        if (!autoAcceptJobId().isEmpty()) {
            m_swap->stopJob(autoAcceptJobId());
        }
    }
    delete m_swap;
}

void SwapUiPlugin::initLogos(LogosAPI* api)
{
    m_logosAPI = api;
    m_swap = new Swap(api);
    setBackend(this);
    setStatus(QStringLiteral("Please choose a configuration."));

    // Ask the swap core module for this profile's persistence root (issue #99).
    // The connection may not be up yet, so this is async: loadConfigFromDisk()
    // / loadReceiptsFromDisk() below run against the best location known right
    // now (LOGOS_USER_DIR, else the legacy shared path as a migration-read),
    // and onPersistenceRootResolved() re-resolves + flushes once the per-profile
    // root arrives. Kicked off before the initial load so that, on the common
    // path where the core is already connected, the root is often known by the
    // time the queued reload runs.
    requestPersistenceRoot();

    // Load any saved config BEFORE the async default fill below, so a saved
    // (possibly user-entered) lez_htlc_program_id wins over the compiled-in
    // default — the async lambda's own isEmpty() guard already does the
    // right thing once this has run first.
    loadConfigFromDisk();

    // Default the maker's LEZ HTLC program-ID field to the canonical value
    // compiled into the Rust library (the public-testnet deployment ID, see
    // swap-ffi/src/lez_htlc_program_id.rs). Only fills when the user hasn't
    // set one (whether that's because nothing was saved, or the saved config
    // simply never had one), so a hand-entered ID always wins; empty-guard
    // kept as a defensive no-op should the library ever ship without a
    // baked-in ID.
    m_swap->defaultLezHtlcProgramIdAsync([this](QString programId) {
        if (!programId.isEmpty()) {
            // Cache the canonical program ID for the offer-venue guard (P0),
            // independently of whether we also fill the (possibly user-set)
            // config field below.
            m_canonicalLezHtlcProgramId = programId;
        }
        if (!programId.isEmpty() && lezHtlcProgramId().isEmpty()) {
            setLezHtlcProgramId(programId);
        }
    });

    swapUiTrace(QStringLiteral("initLogos: starting"));
    loadReceiptsFromDisk();
    const bool subscribed = subscribeToSwapEvents();
    swapUiTrace(QStringLiteral("initLogos: subscribeToSwapEvents=%1").arg(subscribed ? "ok" : "failed"));
    m_messagingPollTimer.start();
    const QString autoEnvFile = qEnvironmentVariable("SWAP_UI_AUTO_ENV_FILE");
    const QString autoRole = qEnvironmentVariable("SWAP_UI_AUTO_ROLE");
    if (!autoEnvFile.isEmpty()) {
        QMetaObject::invokeMethod(this, [this, autoEnvFile, autoRole]() {
            loadEnvFile(autoEnvFile, autoRole);
        }, Qt::QueuedConnection);
    }
    QMetaObject::invokeMethod(this, [this]() {
        startBackgroundServices();
    }, Qt::QueuedConnection);
    qDebug() << "SwapUiPlugin: initialized";
}

QJsonObject SwapUiPlugin::parseObject(const QString& json)
{
    const auto doc = QJsonDocument::fromJson(json.toUtf8());
    return doc.isObject() ? doc.object() : QJsonObject{};
}

QString SwapUiPlugin::compactJson(const QJsonObject& obj)
{
    return QString::fromUtf8(QJsonDocument(obj).toJson(QJsonDocument::Compact));
}

QString SwapUiPlugin::compactJsonValue(const QJsonValue& value)
{
    if (value.isUndefined() || value.isNull()) {
        return QStringLiteral("{}");
    }
    if (value.isObject()) {
        return QString::fromUtf8(QJsonDocument(value.toObject()).toJson(QJsonDocument::Compact));
    }
    if (value.isArray()) {
        return QString::fromUtf8(QJsonDocument(value.toArray()).toJson(QJsonDocument::Compact));
    }
    QJsonObject wrapper;
    wrapper.insert(QStringLiteral("value"), value);
    return compactJson(wrapper);
}

QString SwapUiPlugin::payloadFromArgs(const QVariantList& args)
{
    if (args.isEmpty()) {
        return QStringLiteral("{}");
    }
    return compactVariantJson(args.first());
}

QString SwapUiPlugin::jobIdFromResult(const QString& json)
{
    return parseObject(json).value(QStringLiteral("job_id")).toString();
}

bool SwapUiPlugin::isPositiveInteger(const QString& value)
{
    bool ok = false;
    const auto parsed = value.trimmed().toLongLong(&ok);
    return ok && parsed > 0;
}

bool SwapUiPlugin::isPositiveDecimal(const QString& value)
{
    static const QRegularExpression decimalRe(QStringLiteral(R"(^\d+(\.\d+)?$)"));
    const auto trimmed = value.trimmed();
    if (!decimalRe.match(trimmed).hasMatch()) {
        return false;
    }
    bool ok = false;
    return trimmed.toDouble(&ok) > 0 && ok;
}

bool SwapUiPlugin::isHexBytes(const QString& value, int bytes)
{
    auto clean = value.trimmed();
    if (clean.startsWith(QStringLiteral("0x"), Qt::CaseInsensitive)) {
        clean = clean.mid(2);
    }
    if (clean.size() != bytes * 2) {
        return false;
    }
    static const QRegularExpression hexRe(QStringLiteral(R"(^[0-9a-fA-F]+$)"));
    return hexRe.match(clean).hasMatch();
}

bool SwapUiPlugin::isEthAddress(const QString& value)
{
    return isHexBytes(value, 20);
}

bool SwapUiPlugin::looksLikeBase58(const QString& value)
{
    static const QRegularExpression base58Re(QStringLiteral(R"(^[1-9A-HJ-NP-Za-km-z]+$)"));
    const auto trimmed = value.trimmed();
    return !trimmed.isEmpty() && base58Re.match(trimmed).hasMatch();
}

QString SwapUiPlugin::jsonError(const QString& json)
{
    const auto obj = parseObject(json);
    return obj.value(QStringLiteral("error")).toString();
}

bool SwapUiPlugin::isErrorResult(const QString& json)
{
    return !jsonError(json).isEmpty();
}

QString SwapUiPlugin::configJson() const
{
    QJsonObject obj;
    obj[QStringLiteral("eth_rpc_url")] = ethRpcUrl();
    obj[QStringLiteral("eth_private_key")] = ethPrivateKey();
    obj[QStringLiteral("eth_htlc_address")] = ethHtlcAddress();
    obj[QStringLiteral("lez_sequencer_url")] = lezSequencerUrl();
    if (!lezSigningKey().isEmpty()) {
        obj[QStringLiteral("lez_signing_key")] = lezSigningKey();
    }
    if (!lezWalletHome().isEmpty()) {
        obj[QStringLiteral("lez_wallet_home")] = lezWalletHome();
    }
    if (!lezAccountId().isEmpty()) {
        obj[QStringLiteral("lez_account_id")] = lezAccountId();
    }
    obj[QStringLiteral("lez_htlc_program_id")] = lezHtlcProgramId();
    obj[QStringLiteral("lez_amount")] = lezAmount();
    obj[QStringLiteral("eth_amount")] = ethAmount();
    obj[QStringLiteral("lez_timelock_minutes")] = lezTimelockMinutes();
    obj[QStringLiteral("eth_timelock_minutes")] = ethTimelockMinutes();
    obj[QStringLiteral("eth_recipient_address")] = ethRecipientAddress();
    obj[QStringLiteral("lez_taker_account_id")] = lezTakerAccountId();
    obj[QStringLiteral("poll_interval_ms")] = pollIntervalMs();
    return compactJson(obj);
}

QString SwapUiPlugin::messagingConfigJson() const
{
    QJsonObject obj;
    obj[QStringLiteral("listen_port")] = 0;
    obj[QStringLiteral("portsShift")] = m_deliveryPortsShift;
    return compactJson(obj);
}

void SwapUiPlugin::applyConfigObject(const QJsonObject& obj)
{
    auto setIfPresent = [&](const QString& key, auto setter) {
        if (obj.contains(key)) {
            (this->*setter)(valueString(obj, key));
        }
    };

    setIfPresent(QStringLiteral("eth_rpc_url"), &SwapUiPlugin::setEthRpcUrl);
    setIfPresent(QStringLiteral("eth_private_key"), &SwapUiPlugin::setEthPrivateKey);
    setIfPresent(QStringLiteral("eth_htlc_address"), &SwapUiPlugin::setEthHtlcAddress);
    setIfPresent(QStringLiteral("lez_sequencer_url"), &SwapUiPlugin::setLezSequencerUrl);
    setIfPresent(QStringLiteral("lez_signing_key"), &SwapUiPlugin::setLezSigningKey);
    setIfPresent(QStringLiteral("lez_wallet_home"), &SwapUiPlugin::setLezWalletHome);
    setIfPresent(QStringLiteral("lez_account_id"), &SwapUiPlugin::setLezAccountId);
    setIfPresent(QStringLiteral("lez_htlc_program_id"), &SwapUiPlugin::setLezHtlcProgramId);
    setIfPresent(QStringLiteral("lez_amount"), &SwapUiPlugin::setLezAmount);
    setIfPresent(QStringLiteral("eth_amount"), &SwapUiPlugin::setEthAmount);
    setIfPresent(QStringLiteral("lez_timelock_minutes"), &SwapUiPlugin::setLezTimelockMinutes);
    setIfPresent(QStringLiteral("eth_timelock_minutes"), &SwapUiPlugin::setEthTimelockMinutes);
    setIfPresent(QStringLiteral("eth_recipient_address"), &SwapUiPlugin::setEthRecipientAddress);
    setIfPresent(QStringLiteral("lez_taker_account_id"), &SwapUiPlugin::setLezTakerAccountId);
    setIfPresent(QStringLiteral("poll_interval_ms"), &SwapUiPlugin::setPollIntervalMs);

    if (obj.contains(QStringLiteral("swap_role"))) {
        setRole(valueString(obj, QStringLiteral("swap_role")));
    }

    scheduleConfigSave();
}

// ---------------------------------------------------------------------------
// Config persistence
// ---------------------------------------------------------------------------
//
// Per-profile writable directory for swap_ui's durable state (config.json +
// receipts.jsonl). See the header for the full issue-#99 rationale. Priority:
//   (a) <LOGOS_USER_DIR>/module_data/swap_ui           — explicit-profile
//       (`--user-dir`) launches export LOGOS_USER_DIR into this process, and
//       that path is already profile-correct AND matches the established
//       module_data/swap_ui convention every doc/tool expects. It is preferred
//       when explicitly set so an explicit launch is never disturbed (the task
//       calls these "already correct") — critically, it does NOT relocate the
//       data mid-session once the async core root below arrives.
//   (b) <swap-core instancePersistencePath()>/swap_ui  — the fix for a DEFAULT
//       (no --user-dir) launch, where LOGOS_USER_DIR is UNSET in this
//       out-of-process ui-host. The swap CORE module IS given a correct
//       per-profile path by the host; we anchor under a swap_ui/ subdir of it.
// Returns empty when neither is known yet (default launch, core connection
// still coming up); callers then defer/buffer their writes rather than fall
// through to the shared ui-host tree. This function NEVER returns that tree.
QString SwapUiPlugin::writableModuleDir() const
{
    const QString userDir = qEnvironmentVariable("LOGOS_USER_DIR");
    if (!userDir.isEmpty()) {
        return QDir(userDir).filePath(QStringLiteral("module_data/swap_ui"));
    }
    if (!m_persistenceRoot.isEmpty()) {
        return QDir(m_persistenceRoot).filePath(QStringLiteral("swap_ui"));
    }
    return {};
}

// The pre-fix location: Qt's AppDataLocation, which for the out-of-process
// ui-host resolves under `Logos/ui-host/` — SHARED across every Basecamp
// profile. READ-ONLY: consulted once as a migration fallback by the *ReadPath()
// helpers when the per-profile location has nothing yet, and never written to,
// deleted, or migrated in place (the old shared file is cleaned up manually by
// its owner — matches the upstream Basecamp #315/#316 gate).
QString SwapUiPlugin::legacyModuleDir() const
{
    return QDir(QStandardPaths::writableLocation(QStandardPaths::AppDataLocation))
        .filePath(QStringLiteral("module_data/swap_ui"));
}

// config.json holds every field configJson() serializes — including
// eth_private_key and lez_signing_key, i.e. two private keys — so it is written
// 0600 and replaced atomically (temp file + rename); reads prefer the
// per-profile copy and fall back to the legacy shared file exactly once (only
// when the per-profile location has no config yet).
QString SwapUiPlugin::configReadPath() const
{
    const QString writable = writableModuleDir();
    if (!writable.isEmpty()) {
        const QString p = QDir(writable).filePath(QStringLiteral("config.json"));
        if (QFileInfo::exists(p)) {
            return p;
        }
    }
    return QDir(legacyModuleDir()).filePath(QStringLiteral("config.json"));
}

QString SwapUiPlugin::receiptsReadPath() const
{
    const QString writable = writableModuleDir();
    if (!writable.isEmpty()) {
        const QString p = QDir(writable).filePath(QStringLiteral("receipts.jsonl"));
        if (QFileInfo::exists(p)) {
            return p;
        }
    }
    return QDir(legacyModuleDir()).filePath(QStringLiteral("receipts.jsonl"));
}

// Ask the swap CORE module for its per-profile persistence root. swap_ui, as a
// ui_qml plugin, gets no host identity of its own (no LOGOS_USER_DIR under a
// default launch, no instancePersistencePath — see logos_ui_plugin_context.h),
// so it cannot reconstruct its own profile root. The swap core module runs
// in-process under logos_host and DOES receive a correct per-profile context;
// we query it over the same codegen/QtRO interface swap_ui already uses. The
// connection may not be up yet at initLogos time, so this is async and applied
// in onPersistenceRootResolved() whenever it lands.
void SwapUiPlugin::requestPersistenceRoot()
{
    if (!m_swap) {
        return;
    }
    m_swap->persistenceRootAsync([this](QString root) {
        onPersistenceRootResolved(root.trimmed());
    });
}

void SwapUiPlugin::onPersistenceRootResolved(const QString& root)
{
    if (root.isEmpty()) {
        // The core is running without a host-provisioned persistence dir (e.g.
        // a dev/test host, or lgpd). Keep the LOGOS_USER_DIR/legacy behaviour;
        // if LOGOS_USER_DIR is set, deferred writes can still flush below.
        swapUiTrace(QStringLiteral(
            "onPersistenceRootResolved: core returned empty root; keeping LOGOS_USER_DIR/legacy fallback"));
    } else if (root != m_persistenceRoot) {
        m_persistenceRoot = root;
        // Distinctive release-verification marker: a unique literal that must
        // survive into the built swap_ui .lgx (asserted by the release
        // pipeline). Kept as a plain UTF-8 char[] (not QStringLiteral, which
        // would land in the binary as UTF-16 and defeat an ASCII grep) so a
        // release `grep`/`strings` over the artifact finds it. Do not reword
        // without updating the release grep.
        static const char kPersistenceMarker[] =
            "swap_ui persistence root (per-profile)";
        swapUiTrace(QString::fromUtf8(kPersistenceMarker)
                    + QStringLiteral(": %1").arg(writableModuleDir()));
    }

    const QString baseDir = writableModuleDir();
    if (baseDir.isEmpty()) {
        // Still nothing writable (empty core root AND no LOGOS_USER_DIR); leave
        // deferred/buffered writes pending rather than touch the shared tree.
        return;
    }

    // Re-resolve config now that a per-profile location exists. A profile-local
    // config is authoritative: load it and drop any init-time migration-read /
    // deferred save so we never clobber the profile copy. Otherwise (fresh
    // profile) keep the in-memory config and flush it into the profile.
    if (QFileInfo::exists(QDir(baseDir).filePath(QStringLiteral("config.json")))) {
        m_pendingConfigSave = false;
        loadConfigFromDisk();
    } else if (m_pendingConfigSave) {
        saveConfigToDisk();
    }

    // Receipts: flush anything buffered while the root was unknown into the
    // per-profile journal, then rebuild the surfaced list from disk.
    flushPendingReceipts();
    loadReceiptsFromDisk();
}

// Append receipt lines buffered while the per-profile root was still unknown to
// the per-profile journal. Realistically never non-empty (a swap cannot finish
// in the sub-second before the async root arrives), but kept for correctness so
// no receipt is silently dropped.
void SwapUiPlugin::flushPendingReceipts()
{
    if (m_pendingReceiptLines.isEmpty()) {
        return;
    }
    const QString baseDir = writableModuleDir();
    if (baseDir.isEmpty()) {
        return;
    }
    const QDir dir(baseDir);
    if (!dir.exists() && !dir.mkpath(QStringLiteral("."))) {
        swapUiTrace(QStringLiteral("flushPendingReceipts: mkpath failed for %1")
                        .arg(dir.absolutePath()));
        return;
    }
    QFile f(dir.filePath(QStringLiteral("receipts.jsonl")));
    if (!f.open(QIODevice::WriteOnly | QIODevice::Append)) {
        swapUiTrace(QStringLiteral("flushPendingReceipts: open failed: %1")
                        .arg(f.errorString()));
        return;
    }
    for (const QString& line : m_pendingReceiptLines) {
        f.write(line.toUtf8() + '\n');
    }
    f.flush();
    swapUiTrace(QStringLiteral("flushPendingReceipts: flushed %1 buffered receipt(s)")
                    .arg(m_pendingReceiptLines.size()));
    m_pendingReceiptLines.clear();
}

void SwapUiPlugin::loadConfigFromDisk()
{
    // Note for anyone chasing a "my saved config didn't survive a restart"
    // report: for a real installed app, the most likely explanation is that
    // config.json was simply never written in the first place, not that it
    // was written and then lost. Before the synchronous-save fix below
    // (scheduleConfigSave()), the save was debounced behind a timer and only
    // ever flushed from ~SwapUiPlugin() — but the out-of-process ui-host
    // that hosts this plugin is torn down on Quit by a raw OS termination
    // signal, not a graceful Qt shutdown, so that destructor never ran and
    // the file was never created. (A secondary, since-superseded theory once
    // recorded here was that `lgs basecamp launch <profile>`'s reinstall step
    // was wiping an already-written module_data/swap_ui/ — real for that dev
    // flow, but second-order: it explains a file disappearing, not a file
    // that was never there for a real installed app in the first place.)
    // The open-failed trace below stays valuable for telling these apart:
    // "no config at path" after this fix means either a genuinely fresh
    // profile or something new, not the old silent-never-wrote failure mode.
    const QString path = configReadPath();
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly)) {
        swapUiTrace(QStringLiteral("loadConfigFromDisk: no config at %1 (%2)")
                        .arg(path, f.errorString()));
        return;
    }
    const QByteArray data = f.readAll();
    f.close();
    const auto obj = parseObject(QString::fromUtf8(data));
    if (obj.isEmpty()) {
        swapUiTrace(QStringLiteral("loadConfigFromDisk: %1 exists but parsed empty (%2 bytes)")
                        .arg(path, QString::number(data.size())));
        return;
    }
    // applyConfigObject's own scheduleConfigSave() call at the end will
    // synchronously rewrite this file with the same content a moment later —
    // harmless (idempotent), simpler than threading a "don't save" flag
    // through applyConfigObject for the one caller that doesn't want it.
    applyConfigObject(obj);
    swapUiTrace(QStringLiteral("loadConfigFromDisk: loaded config from %1").arg(path));
}

// Writes SYNCHRONOUSLY — no debounce. The out-of-process ui-host that hosts
// this plugin is torn down on Quit by a raw OS termination signal, not a
// graceful Qt shutdown (~SwapUiPlugin() never runs in that path), so any
// timer-based debounce window is a real data-loss window: whatever changed
// in that window is silently discarded. The payload is a few hundred bytes
// of JSON, so there is no performance reason to defer the write — every call
// here (i.e. every keystroke in a Config field) hits disk immediately.
// installSignalHandlers()/handleUnixSignal() below are an additional
// belt-and-braces flush for the (already-covered) case of a signal landing
// mid-edit.
void SwapUiPlugin::scheduleConfigSave()
{
    saveConfigToDisk();
}

void SwapUiPlugin::saveConfigToDisk()
{
    const QString baseDir = writableModuleDir();
    if (baseDir.isEmpty()) {
        // Per-profile root not resolved yet (core connection still coming up
        // and no explicit LOGOS_USER_DIR). Defer rather than write two private
        // keys into the shared ui-host tree; onPersistenceRootResolved()
        // flushes this once the root arrives.
        m_pendingConfigSave = true;
        swapUiTrace(QStringLiteral(
            "saveConfigToDisk: deferring — per-profile persistence root not resolved yet"));
        return;
    }
    m_pendingConfigSave = false;
    const QString path = QDir(baseDir).filePath(QStringLiteral("config.json"));
    const QDir dir = QFileInfo(path).dir();
    if (!dir.exists() && !dir.mkpath(QStringLiteral("."))) {
        swapUiTrace(QStringLiteral("saveConfigToDisk: mkpath failed for %1")
                        .arg(dir.absolutePath()));
        return;
    }

    // Atomic replace: write to a sibling temp file, then rename over the
    // real path. A crash/kill mid-write leaves the old config.json intact
    // (or the temp file orphaned) rather than a half-written, corrupt file.
    const QString tmpPath = path + QStringLiteral(".tmp");
    QFile tmp(tmpPath);
    if (!tmp.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        swapUiTrace(QStringLiteral("saveConfigToDisk: open failed for %1: %2")
                        .arg(tmpPath, tmp.errorString()));
        return;
    }
    tmp.write(configJson().toUtf8());
    tmp.flush();
    tmp.close();
    // 0600: this file holds eth_private_key and lez_signing_key in the
    // clear. Set before the rename so the destination is never briefly
    // group/other-readable under the old name.
    QFile::setPermissions(tmpPath,
                          QFileDevice::ReadOwner | QFileDevice::WriteOwner);

    QFile::remove(path); // QFile::rename fails if the destination exists.
    if (!QFile::rename(tmpPath, path)) {
        swapUiTrace(QStringLiteral("saveConfigToDisk: rename failed %1 -> %2")
                        .arg(tmpPath, path));
        return;
    }
    swapUiTrace(QStringLiteral("saveConfigToDisk: wrote %1").arg(path));
}

// Raw signal handler: async-signal-safe ONLY. write() on a pipe/socket fd is
// one of the few functions POSIX guarantees is safe to call here; nothing
// Qt-related (no QFile, no QString, no logging) may run in this context. All
// it does is wake the QSocketNotifier set up in installSignalHandlers() by
// writing the signal number to the self-pipe.
void SwapUiPlugin::unixSignalHandler(int sig)
{
    const char byte = static_cast<char>(sig);
    // Return value deliberately ignored: nothing safe to do with a failed
    // write() from inside a signal handler, and the pipe is tiny/non-blocking
    // so a full buffer is the only realistic failure.
    ssize_t written = ::write(g_signalSelfPipe[1], &byte, sizeof(byte));
    (void)written;
}

// Standard Qt self-pipe pattern: the raw signal handler above only writes a
// byte; this sets up the socketpair + QSocketNotifier that lets the actual
// (Qt-safe) flush run on the main thread's event loop instead of in signal
// context. Captures the previous SIGTERM/SIGINT disposition so
// handleUnixSignal() can restore and re-deliver it after flushing, rather
// than silently swallowing whatever ui-host's own handler was doing (its own
// shutdown logging/sequencing — see the "received termination signal"
// message this was written to withstand).
void SwapUiPlugin::installSignalHandlers()
{
    if (::socketpair(AF_UNIX, SOCK_STREAM, 0, g_signalSelfPipe) != 0) {
        swapUiTrace(QStringLiteral("installSignalHandlers: socketpair failed"));
        return;
    }
    ::fcntl(g_signalSelfPipe[1], F_SETFL, O_NONBLOCK);

    m_signalNotifier = new QSocketNotifier(g_signalSelfPipe[0], QSocketNotifier::Read, this);
    connect(m_signalNotifier, &QSocketNotifier::activated,
            this, &SwapUiPlugin::handleUnixSignal);

    struct sigaction action {};
    action.sa_handler = &SwapUiPlugin::unixSignalHandler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART;
    sigaction(SIGTERM, &action, &g_prevSigTerm);
    sigaction(SIGINT, &action, &g_prevSigInt);
    swapUiTrace(QStringLiteral("installSignalHandlers: SIGTERM/SIGINT handlers installed"));
}

void SwapUiPlugin::handleUnixSignal()
{
    m_signalNotifier->setEnabled(false);
    char byte = 0;
    const auto n = ::read(g_signalSelfPipe[0], &byte, sizeof(byte));
    const int sig = (n == sizeof(byte)) ? static_cast<int>(byte) : SIGTERM;

    swapUiTrace(QStringLiteral("handleUnixSignal: signal %1 received, flushing config").arg(sig));
    // Config saves are already synchronous (scheduleConfigSave()); this call
    // is the belt-and-braces net for anything that slipped through — cheap
    // and idempotent, so unconditional.
    saveConfigToDisk();

    // Chain to whatever disposition was previously installed (ui-host's own
    // handler, or the OS default) so its shutdown sequencing still happens —
    // we only ever want to run BEFORE it, never instead of it.
    if (sig == SIGTERM) {
        sigaction(SIGTERM, &g_prevSigTerm, nullptr);
    } else if (sig == SIGINT) {
        sigaction(SIGINT, &g_prevSigInt, nullptr);
    }
    ::raise(sig);
}

void SwapUiPlugin::resetConfig()
{
    // Only ever remove the per-profile config; never delete the legacy shared
    // file (issue #99 / Basecamp #315 gate — the old shared file is cleaned up
    // manually by its owner). applyDefaultConfig()'s save below writes defaults
    // into the per-profile location, which then shadows any legacy file for
    // subsequent reads.
    const QString baseDir = writableModuleDir();
    if (baseDir.isEmpty()) {
        swapUiTrace(QStringLiteral(
            "resetConfig: per-profile root unknown; resetting in-memory config only"));
    } else {
        const QString path = QDir(baseDir).filePath(QStringLiteral("config.json"));
        if (QFile::exists(path) && !QFile::remove(path)) {
            swapUiTrace(QStringLiteral("resetConfig: failed to remove %1").arg(path));
        }
    }
    applyDefaultConfig();
    validateConfig();
    if (m_swap) {
        m_swap->defaultLezHtlcProgramIdAsync([this](QString programId) {
            if (!programId.isEmpty()) {
                m_canonicalLezHtlcProgramId = programId;
            }
            if (!programId.isEmpty() && lezHtlcProgramId().isEmpty()) {
                setLezHtlcProgramId(programId);
            }
        });
    }
    setErrorMessage(QString{});
    setStatus(QStringLiteral("App data reset to defaults"));
}

// ---------------------------------------------------------------------------
// Guided first-run Setup (SetupView.qml)
// ---------------------------------------------------------------------------
//
// Generation is instant/local (no network) and writes straight into the
// existing eth*/lez* config PROPs through the SAME setConfigValue-style
// path manual entry uses (validateConfig + scheduleConfigSave, which now
// saves synchronously and chmod 0600s the file, see #89) — so a generated
// key is persisted exactly like a pasted one, and the rest of the app can't
// tell the difference.

void SwapUiPlugin::setupGenerateEthKey()
{
    if (!m_swap) {
        setErrorMessage(QStringLiteral("Swap client not ready"));
        setStatus(errorMessage());
        return;
    }
    m_swap->generateEthKeyAsync([this](QString result) {
        const auto error = jsonError(result);
        if (!error.isEmpty()) {
            setSetupError(error);
            setStatus(QStringLiteral("Failed to generate ETH key: %1").arg(error));
            return;
        }
        const auto obj = parseObject(result);
        // A taker's recipient is its own address: it locks ETH and later
        // claims it back to itself (or is refunded to itself), so the
        // generated key's own address is always the right default here.
        setEthPrivateKey(valueString(obj, QStringLiteral("private_key")));
        setEthRecipientAddress(valueString(obj, QStringLiteral("address")));
        setSetupError(QString{});
        validateConfig();
        scheduleConfigSave();
        fetchBalances();
    });
}

void SwapUiPlugin::setupGenerateLezAccount()
{
    if (!m_swap) {
        setErrorMessage(QStringLiteral("Swap client not ready"));
        setStatus(errorMessage());
        return;
    }
    m_swap->generateLezAccountAsync([this](QString result) {
        const auto error = jsonError(result);
        if (!error.isEmpty()) {
            setSetupError(error);
            setStatus(QStringLiteral("Failed to generate LEZ account: %1").arg(error));
            return;
        }
        const auto obj = parseObject(result);
        // Only the signing key is a config field (raw-key auth derives its
        // account ID from it) — lezAccountId is the WALLET-mode field
        // (paired with lezWalletHome) and stays untouched here.
        setLezSigningKey(valueString(obj, QStringLiteral("signing_key")));
        // The FFI result already carries the derived base58 account ID, so
        // surface it immediately instead of waiting on fetchBalances' round
        // trip (which historically never ran on a fresh install because
        // validation hard-required lez_taker_account_id; that requirement is
        // gone — see validateConfig — but the direct display is still the
        // right fix for SetupView's "Account: (refreshing…)").
        //
        // Deliberately NOT auto-filled into lez_taker_account_id: that field
        // is the maker-side designated-counterparty ALLOWLIST (see
        // src/config.rs SwapConfig::lez_taker_account_id) — the taker never
        // reads it (it signs with its own account, src/swap/taker.rs), and a
        // maker rejects a lock naming its own account. Auto-filling the
        // user's own account here would make their maker reject every real
        // taker with NotDesignatedTaker.
        setLezAccount(valueString(obj, QStringLiteral("account_id")));
        setSetupError(QString{});
        validateConfig();
        scheduleConfigSave();
        fetchBalances();
    });
}

void SwapUiPlugin::setupStartFunding()
{
    if (!m_swap || setupRunning()) {
        return;
    }
    if (lezSequencerUrl().trimmed().isEmpty() || lezSigningKey().trimmed().isEmpty()) {
        setSetupError(QStringLiteral("Generate or import a LEZ account before funding"));
        setStatus(setupError());
        return;
    }

    setSetupRunning(true);
    setSetupJobId(QString{});
    setSetupError(QString{});
    setSetupStep(QStringLiteral("Initializing"));
    setSetupBalance(QString{});
    setSetupClaims(0);
    // 150 LEZ: exactly one native pinata claim. A taker mostly RECEIVES LEZ
    // (it locks ETH and claims LEZ a maker already escrowed), so onboarding
    // needs the account to exist and be initialized far more than it needs a
    // big balance — see the fuller rationale on
    // SwapImpl::startLezFundingJob / kDefaultSetupFundingTargetLez, which
    // this mirrors so the two can't drift.
    setSetupTarget(QStringLiteral("150"));
    setStatus(QStringLiteral("Setting up your LEZ account..."));

    m_swap->startLezFundingJobAsync(lezSequencerUrl(), lezSigningKey(), setupTarget(),
                                    [this](QString result) {
        const auto error = jsonError(result);
        if (!error.isEmpty()) {
            setSetupError(error);
            setStatus(QStringLiteral("Failed to start LEZ setup: %1").arg(error));
            setSetupRunning(false);
            return;
        }
        // Late start-ack guard, mirroring handleJobStartResult's maker/taker
        // handling: a fast lez_setup.finished may have already landed and
        // cleared setupRunning before this ack arrived.
        if (!setupRunning()) {
            return;
        }
        setSetupJobId(jobIdFromResult(result));
    });
}

QString SwapUiPlugin::canonicalLezHtlcProgramId() const
{
    // The library-baked value cached from defaultLezHtlcProgramIdAsync. Before
    // it arrives (rare — it is resolved at initLogos, long before any offer can
    // be accepted), fall back to the current config field, which
    // applyDefaultConfig / the async fill pinned to the same canonical value
    // and which applyOfferObject deliberately never overwrites from an offer.
    return m_canonicalLezHtlcProgramId.isEmpty() ? lezHtlcProgramId()
                                                 : m_canonicalLezHtlcProgramId;
}

bool SwapUiPlugin::offerNamesCanonicalVenue(const QJsonObject& offer, QString* reason) const
{
    const swap_ui::VenueCheck check = swap_ui::checkOfferVenue(
        valueString(offer, QStringLiteral("eth_htlc_address")).toStdString(),
        defaultEthHtlcAddress().toStdString(),
        valueString(offer, QStringLiteral("lez_htlc_program_id")).toStdString(),
        canonicalLezHtlcProgramId().toStdString());
    if (!check.ok) {
        if (reason) {
            *reason = QString::fromStdString(check.reason);
        }
        return false;
    }
    return true;
}

bool SwapUiPlugin::applyOfferObject(const QJsonObject& offer)
{
    // P0 (fund-theft): the swap venue an offer names — the ETH HTLC contract and
    // the LEZ HTLC program — is NOT taker-configurable per offer. It must equal
    // the app's canonical/pinned deployment. A malicious maker who advertised
    // its OWN contract/program could steer this taker onto a venue it controls,
    // learn the preimage the taker reveals there, and sweep the taker's real
    // ETH. Refuse such an offer outright and mutate NOTHING.
    QString reason;
    if (!offerNamesCanonicalVenue(offer, &reason)) {
        setErrorMessage(reason);
        setStatus(errorMessage());
        return false;
    }

    // Only the economic terms and the maker's recipient address are adopted from
    // the offer. The recipient (maker_eth_address) legitimately varies per maker
    // and is bound into the ETH swapId anyway. The venue fields (eth_htlc_address,
    // lez_htlc_program_id) are deliberately NOT copied — they stay pinned to the
    // app's canonical values (validated above and re-asserted in
    // validateConfigForAction("taker")). The hashlock is taker-generated.
    setEthRecipientAddress(valueString(offer, QStringLiteral("maker_eth_address")));
    setLezAmount(valueString(offer, QStringLiteral("lez_amount")));
    setEthAmount(weiToEthValue(valueString(offer, QStringLiteral("eth_amount"))));

    // Adopt the offer's own timelocks instead of leaving this taker profile's
    // (possibly stale/default) minutes in place. The offer carries absolute
    // unix expiries (lez_timelock/eth_timelock); the maker-loop's runtime gate
    // requires eth_expiry >= fresh_lez_expiry + margin + transit slack (see
    // src/swap/maker.rs classify_candidate + src/cli/bot.rs
    // validate_timelocks). A taker whose config disagrees with the advertised
    // expiries loses that race on every candidate, wedging the maker at
    // WaitingForEthLock forever. The absolute-expiry -> minutes-from-now
    // conversion (ceil, clamped to >=1) lives in timelock_math.h so it has a
    // standalone unit test (tests/timelock_math_test.cpp) independent of the
    // full plugin/module build.
    const qint64 nowSecs = QDateTime::currentSecsSinceEpoch();
    const qint64 ethExpirySecs =
        static_cast<qint64>(offer.value(QStringLiteral("eth_timelock")).toDouble(0));
    const qint64 lezExpirySecs =
        static_cast<qint64>(offer.value(QStringLiteral("lez_timelock")).toDouble(0));
    if (ethExpirySecs > 0) {
        setEthTimelockMinutes(
            QString::number(swap_ui::minutesUntilExpiry(ethExpirySecs, nowSecs)));
    }
    if (lezExpirySecs > 0) {
        setLezTimelockMinutes(
            QString::number(swap_ui::minutesUntilExpiry(lezExpirySecs, nowSecs)));
    }
    return true;
}

void SwapUiPlugin::addValidationError(QJsonObject& errors,
                                      const QString& key,
                                      const QString& message) const
{
    errors.insert(key, message);
}

bool SwapUiPlugin::validateConfig()
{
    QJsonObject errors;

    auto require = [&](const QString& key, const QString& value) {
        if (value.trimmed().isEmpty()) {
            addValidationError(errors, key, QStringLiteral("Required"));
        }
    };

    require(QStringLiteral("eth_rpc_url"), ethRpcUrl());
    require(QStringLiteral("eth_private_key"), ethPrivateKey());
    require(QStringLiteral("eth_htlc_address"), ethHtlcAddress());
    require(QStringLiteral("lez_sequencer_url"), lezSequencerUrl());
    require(QStringLiteral("lez_htlc_program_id"), lezHtlcProgramId());
    require(QStringLiteral("lez_amount"), lezAmount());
    require(QStringLiteral("eth_amount"), ethAmount());
    require(QStringLiteral("eth_recipient_address"), ethRecipientAddress());
    // lez_taker_account_id is deliberately NOT required: it is the OPTIONAL
    // maker-side designated-counterparty allowlist (empty = serve any taker;
    // swap-ffi maps an empty string to None). The taker never reads it — it
    // signs with its own account. Requiring it forced first-time users to
    // hand-paste an ID into a field they should normally leave blank, and
    // pasting their OWN account makes their maker reject every counterparty.
    require(QStringLiteral("poll_interval_ms"), pollIntervalMs());
    require(QStringLiteral("lez_timelock_minutes"), lezTimelockMinutes());
    require(QStringLiteral("eth_timelock_minutes"), ethTimelockMinutes());

    if (!ethPrivateKey().trimmed().isEmpty() && !isHexBytes(ethPrivateKey(), 32)) {
        addValidationError(errors, QStringLiteral("eth_private_key"), QStringLiteral("Must be a 32-byte hex key"));
    }
    if (!ethHtlcAddress().trimmed().isEmpty() && !isEthAddress(ethHtlcAddress())) {
        addValidationError(errors, QStringLiteral("eth_htlc_address"), QStringLiteral("Must be a 20-byte ETH address"));
    }
    if (!ethRecipientAddress().trimmed().isEmpty() && !isEthAddress(ethRecipientAddress())) {
        addValidationError(errors, QStringLiteral("eth_recipient_address"), QStringLiteral("Must be a 20-byte ETH address"));
    }
    if (!lezHtlcProgramId().trimmed().isEmpty() && !isHexBytes(lezHtlcProgramId(), 32)) {
        addValidationError(errors, QStringLiteral("lez_htlc_program_id"), QStringLiteral("Must be a 32-byte hex program ID"));
    }
    if (!lezSigningKey().trimmed().isEmpty() && !isHexBytes(lezSigningKey(), 32)) {
        addValidationError(errors, QStringLiteral("lez_signing_key"), QStringLiteral("Must be a 32-byte hex key"));
    }

    const bool hasRawKey = !lezSigningKey().trimmed().isEmpty();
    const bool hasWalletHome = !lezWalletHome().trimmed().isEmpty();
    const bool hasAccountId = !lezAccountId().trimmed().isEmpty();
    if (!hasRawKey && !(hasWalletHome && hasAccountId)) {
        addValidationError(errors, QStringLiteral("lez_signing_key"),
                           QStringLiteral("Set a signing key or wallet home plus account ID"));
        if (!hasWalletHome) {
            addValidationError(errors, QStringLiteral("lez_wallet_home"), QStringLiteral("Required with wallet auth"));
        }
        if (!hasAccountId) {
            addValidationError(errors, QStringLiteral("lez_account_id"), QStringLiteral("Required with wallet auth"));
        }
    }
    if (hasWalletHome && !hasAccountId) {
        addValidationError(errors, QStringLiteral("lez_account_id"), QStringLiteral("Required with wallet home"));
    }
    if (!hasWalletHome && hasAccountId && !hasRawKey) {
        addValidationError(errors, QStringLiteral("lez_wallet_home"), QStringLiteral("Required with account ID"));
    }
    if (hasAccountId && !looksLikeBase58(lezAccountId())) {
        addValidationError(errors, QStringLiteral("lez_account_id"), QStringLiteral("Must be base58"));
    }
    if (!lezTakerAccountId().trimmed().isEmpty() && !looksLikeBase58(lezTakerAccountId())) {
        addValidationError(errors, QStringLiteral("lez_taker_account_id"), QStringLiteral("Must be base58"));
    }
    if (!isPositiveInteger(lezAmount())) {
        addValidationError(errors, QStringLiteral("lez_amount"), QStringLiteral("Must be a positive integer"));
    }
    if (!isPositiveDecimal(ethAmount())) {
        addValidationError(errors, QStringLiteral("eth_amount"), QStringLiteral("Must be a positive decimal"));
    }
    if (!isPositiveInteger(lezTimelockMinutes())) {
        addValidationError(errors, QStringLiteral("lez_timelock_minutes"), QStringLiteral("Must be positive minutes"));
    }
    if (!isPositiveInteger(ethTimelockMinutes())) {
        addValidationError(errors, QStringLiteral("eth_timelock_minutes"), QStringLiteral("Must be positive minutes"));
    }
    if (isPositiveInteger(lezTimelockMinutes()) && isPositiveInteger(ethTimelockMinutes())
        && ethTimelockMinutes().toLongLong() <= lezTimelockMinutes().toLongLong()) {
        addValidationError(errors, QStringLiteral("eth_timelock_minutes"),
                           QStringLiteral("Must be greater than LEZ timelock"));
    }
    if (!isPositiveInteger(pollIntervalMs())) {
        addValidationError(errors, QStringLiteral("poll_interval_ms"), QStringLiteral("Must be a positive interval"));
    }
    setValidationErrorsJson(compactJson(errors));
    return errors.isEmpty();
}

bool SwapUiPlugin::validateConfigForAction(const QString& action,
                                           const QString& hexValue,
                                           const QString& hexKey)
{
    bool ok = validateConfig();
    auto errors = parseObject(validationErrorsJson());

    if (!hexKey.isEmpty()) {
        if (action.startsWith(QStringLiteral("refund")) && hexValue.trimmed().isEmpty()) {
            addValidationError(errors, hexKey, QStringLiteral("Required"));
            ok = false;
        } else if (!hexValue.trimmed().isEmpty() && !isHexBytes(hexValue, 32)) {
            addValidationError(errors, hexKey, QStringLiteral("Must be 32 bytes of hex"));
            ok = false;
        }
    }

    // P0 (fund-theft): the taker only ever trades on the app's canonical, pinned
    // venue. Re-assert it here as a config-level invariant — independent of any
    // single offer — so a config whose eth_htlc_address / lez_htlc_program_id was
    // tampered with (e.g. an offer path that somehow bypassed applyOfferObject,
    // or a hand-edited config) can never start a taker swap on an attacker's
    // contract/program.
    if (action == QStringLiteral("taker")) {
        if (!swap_ui::hexEquals(ethHtlcAddress().toStdString(),
                                defaultEthHtlcAddress().toStdString())) {
            addValidationError(errors, QStringLiteral("eth_htlc_address"),
                               QStringLiteral("Must be the app's canonical ETH HTLC contract"));
            ok = false;
        }
        if (!swap_ui::hexEquals(lezHtlcProgramId().toStdString(),
                                canonicalLezHtlcProgramId().toStdString())) {
            addValidationError(errors, QStringLiteral("lez_htlc_program_id"),
                               QStringLiteral("Must be the app's canonical LEZ HTLC program"));
            ok = false;
        }
    }

    setValidationErrorsJson(compactJson(errors));
    if (!ok) {
        setErrorMessage(QStringLiteral("Fix validation errors before continuing"));
        setStatus(errorMessage());
    }
    return ok;
}

// The lightweight gate for fetchOffers(): only the network
// endpoints/constants needed to reach Delivery and read the offer feed.
// Deliberately excludes credentials (eth_private_key, lez signing/wallet),
// trade amounts, and timelocks — those only matter once a specific offer is
// being accepted (see validateForTrade / canAccept in OfferBoard.qml).
bool SwapUiPlugin::validateForBrowse() const
{
    return !ethRpcUrl().trimmed().isEmpty()
        && !ethHtlcAddress().trimmed().isEmpty()
        && isEthAddress(ethHtlcAddress())
        && !lezSequencerUrl().trimmed().isEmpty()
        && !lezHtlcProgramId().trimmed().isEmpty()
        && isHexBytes(lezHtlcProgramId(), 32);
}

// Named alias for validateConfig(): the full check every trade action
// (accept, publish, auto-accept, refund) requires. Kept as a distinct name
// so call sites read as "browse" vs. "trade" intent rather than both saying
// the same ambiguous "validateConfig".
bool SwapUiPlugin::validateForTrade()
{
    return validateConfig();
}

void SwapUiPlugin::updateRunning()
{
    setRunning(makerRunning() || takerRunning() || autoAcceptRunning());
}

void SwapUiPlugin::setBusyState()
{
    setErrorMessage(QString{});
    updateRunning();
}

void SwapUiPlugin::clearMakerProgress()
{
    setMakerProgressSteps(QStringList{});
    setMakerCurrentStep(QString{});
    setMakerResultJson(QString{});
}

void SwapUiPlugin::clearTakerProgress()
{
    setTakerProgressSteps(QStringList{});
    setTakerCurrentStep(QString{});
    setTakerResultJson(QString{});
}

void SwapUiPlugin::addMakerProgressStep(const QString& step)
{
    auto steps = makerProgressSteps();
    if (!step.isEmpty() && !steps.contains(step)) {
        steps.append(step);
        setMakerProgressSteps(steps);
    }
}

void SwapUiPlugin::addTakerProgressStep(const QString& step)
{
    auto steps = takerProgressSteps();
    if (!step.isEmpty() && !steps.contains(step)) {
        steps.append(step);
        setTakerProgressSteps(steps);
    }
}

bool SwapUiPlugin::shouldHandleJobEvent(const QString& eventName, const QJsonObject& payload) const
{
    const auto jobId = valueString(payload, QStringLiteral("job_id"));
    QString activeJobId;
    QString activeKind;
    if (eventName.startsWith(QStringLiteral("maker_loop."))) {
        activeJobId = autoAcceptJobId();
        activeKind = QStringLiteral("autoAcceptJobId");
    } else if (eventName.startsWith(QStringLiteral("maker."))) {
        activeJobId = makerJobId();
        activeKind = QStringLiteral("makerJobId");
    } else if (eventName.startsWith(QStringLiteral("taker."))) {
        activeJobId = takerJobId();
        activeKind = QStringLiteral("takerJobId");
    }

    const auto step = valueString(payload, QStringLiteral("step"));
    const auto payloadRole = valueString(payload, QStringLiteral("role"));

    if (eventName.startsWith(QStringLiteral("lez_setup."))) {
        activeJobId = setupJobId();
        activeKind = QStringLiteral("setupJobId");
    }

    QString expectedRole;
    bool active = false;
    if (eventName.startsWith(QStringLiteral("maker_loop."))) {
        expectedRole = QStringLiteral("maker_loop");
        active = autoAcceptRunning();
    } else if (eventName.startsWith(QStringLiteral("maker."))) {
        expectedRole = QStringLiteral("maker");
        active = makerRunning();
    } else if (eventName.startsWith(QStringLiteral("taker."))) {
        expectedRole = QStringLiteral("taker");
        active = takerRunning();
    } else if (eventName.startsWith(QStringLiteral("lez_setup."))) {
        expectedRole = QStringLiteral("lez_setup");
        active = setupRunning();
    } else {
        swapUiTrace(QStringLiteral("DROP event=%1 reason=unknown_event_name").arg(eventName));
        return false;
    }

    if (!active) {
        swapUiTrace(QStringLiteral("DROP event=%1 step=%2 reason=role_not_active activeKind=%3 payload.job_id=%4")
                        .arg(eventName, step, activeKind, jobId));
        return false;
    }

    if (!payloadRole.isEmpty() && payloadRole != expectedRole) {
        swapUiTrace(QStringLiteral("DROP event=%1 step=%2 reason=role_mismatch payload.role=%3 expected=%4")
                        .arg(eventName, step, payloadRole, expectedRole));
        return false;
    }

    // Role-first / job_id-second: tolerate the early-event race where the
    // swap-module's detached worker thread starts emitting *.progress events
    // before startMakerLoopJobAsync's response (carrying the job_id) has
    // landed in handleJobStartResult. Once the UI knows the active job_id,
    // we enforce equality to reject stale events from a previous run.
    if (!activeJobId.isEmpty() && !jobId.isEmpty() && jobId != activeJobId) {
        swapUiTrace(QStringLiteral("DROP event=%1 step=%2 reason=job_id_mismatch payload.job_id=%3 expected=%4")
                        .arg(eventName, step, jobId, activeJobId));
        return false;
    }

    swapUiTrace(QStringLiteral("ACCEPT event=%1 step=%2 payload.job_id=%3 active=%4")
                    .arg(eventName, step, jobId, activeJobId));
    return true;
}

void SwapUiPlugin::setResultStatus(const QString& resultJson,
                                   const QString& successStatus,
                                   const QString& failureStatus)
{
    setLastResultJson(resultJson);
    const auto error = jsonError(resultJson);
    if (!error.isEmpty()) {
        setErrorMessage(error);
        setStatus(QStringLiteral("%1: %2").arg(failureStatus, error));
        return;
    }
    setErrorMessage(QString{});
    setStatus(successStatus);
}

void SwapUiPlugin::setRole(const QString& role)
{
    if (role != QStringLiteral("maker") && role != QStringLiteral("taker")) {
        setErrorMessage(QStringLiteral("Invalid role: %1").arg(role));
        setStatus(errorMessage());
        return;
    }
    setSwapRole(role);
    setErrorMessage(QString{});
    setStatus(QStringLiteral("Role: %1").arg(role));
}

void SwapUiPlugin::setConfigValue(const QString& key, const QString& value)
{
    if (key == QStringLiteral("eth_rpc_url")) setEthRpcUrl(value);
    else if (key == QStringLiteral("eth_private_key")) setEthPrivateKey(value);
    else if (key == QStringLiteral("eth_htlc_address")) setEthHtlcAddress(value);
    else if (key == QStringLiteral("lez_sequencer_url")) setLezSequencerUrl(value);
    else if (key == QStringLiteral("lez_signing_key")) setLezSigningKey(value);
    else if (key == QStringLiteral("lez_wallet_home")) setLezWalletHome(value);
    else if (key == QStringLiteral("lez_account_id")) setLezAccountId(value);
    else if (key == QStringLiteral("lez_htlc_program_id")) setLezHtlcProgramId(value);
    else if (key == QStringLiteral("lez_amount")) setLezAmount(value);
    else if (key == QStringLiteral("eth_amount")) setEthAmount(value);
    else if (key == QStringLiteral("lez_timelock_minutes")) setLezTimelockMinutes(value);
    else if (key == QStringLiteral("eth_timelock_minutes")) setEthTimelockMinutes(value);
    else if (key == QStringLiteral("eth_recipient_address")) setEthRecipientAddress(value);
    else if (key == QStringLiteral("lez_taker_account_id")) setLezTakerAccountId(value);
    else if (key == QStringLiteral("poll_interval_ms")) setPollIntervalMs(value);
    else {
        setErrorMessage(QStringLiteral("Unknown config key: %1").arg(key));
        setStatus(errorMessage());
        return;
    }
    validateConfig();
    scheduleConfigSave();
}

void SwapUiPlugin::loadConfig(const QString& configJson)
{
    const auto obj = parseObject(configJson);
    if (obj.isEmpty() && !configJson.trimmed().isEmpty()) {
        setErrorMessage(QStringLiteral("Invalid config JSON"));
        setStatus(errorMessage());
        return;
    }
    applyConfigObject(obj);
    setErrorMessage(QString{});
    setStatus(QStringLiteral("Config loaded"));
    validateConfig();
}

void SwapUiPlugin::loadEnvFile(const QString& path, const QString& role)
{
    if (!m_swap || balancesLoading() || messagingLoading() || offersLoading()
        || publishingLoading() || refundsLoading() || running()) {
        return;
    }

    const QString resolvedPath = resolveLocalEnvPath(path);

    if (!role.isEmpty()) {
        setStatus(QStringLiteral("Loading %1 env...").arg(role));
    } else {
        setStatus(QStringLiteral("Loading %1...").arg(resolvedPath));
    }
    setErrorMessage(QString{});
    m_swap->loadEnvAsync(resolvedPath, [this, role, resolvedPath](QString result) {
        const auto error = jsonError(result);
        if (!error.isEmpty()) {
            setErrorMessage(error);
            setStatus(QStringLiteral("Env load failed: %1").arg(error));
            return;
        }
        applyConfigObject(parseObject(result));
        if (!role.isEmpty()) {
            setRole(role);
        }
        const bool configValid = validateConfig();
        m_loadedEnvPath = resolvedPath;
        setStatus(QStringLiteral("Config loaded from env"));
        if (configValid) {
            fetchBalancesFromLoadedEnv();
        }
    });
}

void SwapUiPlugin::fetchBalancesFromLoadedEnv()
{
    if (!m_swap || m_loadedEnvPath.isEmpty() || balancesLoading()) {
        return;
    }

    setErrorMessage(QString{});
    setBalancesLoading(true);
    setStatus(QStringLiteral("Fetching balances..."));
    m_swap->fetchBalancesFromEnvAsync(m_loadedEnvPath, [this](QString result) {
        completeBalanceRefresh(result);
    });
}

void SwapUiPlugin::requestAutomaticBalanceRefresh()
{
    using Decision = BalanceRefreshCoordinator::Decision;
    const auto decision = m_balanceRefreshCoordinator.requestAutomatic(
        !m_loadedEnvPath.isEmpty(), balancesLoading());
    if (decision == Decision::Start) {
        fetchBalancesFromLoadedEnv();
    } else if (decision == Decision::Unavailable) {
        swapUiTrace(QStringLiteral("automatic balance refresh skipped — no loaded env source"));
    }
}

void SwapUiPlugin::completeBalanceRefresh(const QString& resultJson)
{
    applyBalancesResult(resultJson);
    if (m_balanceRefreshCoordinator.finish(!m_loadedEnvPath.isEmpty())) {
        // Keep balancesLoading true across the coalesced follow-up. Otherwise
        // the header briefly re-enables and an older pre-sale result can look
        // authoritative before the post-sale refresh starts.
        setErrorMessage(QString{});
        setStatus(QStringLiteral("Fetching balances..."));
        m_swap->fetchBalancesFromEnvAsync(m_loadedEnvPath, [this](QString result) {
            completeBalanceRefresh(result);
        });
        return;
    }
    setBalancesLoading(false);
}

void SwapUiPlugin::applyBalancesResult(const QString& resultJson)
{
    setLastResultJson(resultJson);

    const auto error = jsonError(resultJson);
    if (!error.isEmpty()) {
        setErrorMessage(error);
        setStatus(QStringLiteral("Balance fetch failed: %1").arg(error));
        return;
    }

    const auto obj = parseObject(resultJson);
    if (obj.contains(QStringLiteral("eth_address"))) setEthAddress(valueString(obj, QStringLiteral("eth_address")));
    if (obj.contains(QStringLiteral("eth_balance"))) setEthBalance(valueString(obj, QStringLiteral("eth_balance")));
    if (obj.contains(QStringLiteral("lez_account"))) setLezAccount(valueString(obj, QStringLiteral("lez_account")));
    if (obj.contains(QStringLiteral("lez_balance"))) setLezBalance(valueString(obj, QStringLiteral("lez_balance")));

    QStringList warnings;
    const auto ethError = valueString(obj, QStringLiteral("eth_error"));
    const auto lezError = valueString(obj, QStringLiteral("lez_error"));
    if (!ethError.isEmpty()) warnings.append(QStringLiteral("ETH: %1").arg(ethError));
    if (!lezError.isEmpty()) warnings.append(QStringLiteral("LEZ: %1").arg(lezError));

    if (!warnings.isEmpty()) {
        setErrorMessage(warnings.join(QStringLiteral("; ")));
        setStatus(QStringLiteral("Balances fetched with warnings"));
        return;
    }

    setErrorMessage(QString{});
    setStatus(QStringLiteral("Balances fetched"));
}

void SwapUiPlugin::fetchBalances()
{
    if (!m_swap || balancesLoading()) {
        setStatus(QStringLiteral("Swap client not ready"));
        return;
    }
    if (!validateConfig()) {
        setErrorMessage(QStringLiteral("Fix validation errors before fetching balances"));
        setStatus(errorMessage());
        return;
    }

    setErrorMessage(QString{});
    setBalancesLoading(true);
    setStatus(QStringLiteral("Fetching balances..."));
    m_swap->fetchBalancesAsync(configJson(), [this](QString result) {
        completeBalanceRefresh(result);
    });
}

void SwapUiPlugin::handleMakerFinished(const QString& resultJson)
{
    // Journal before any state resets: the receipt merges the authoritative
    // result JSON with the evidence accumulated from progress events.
    journalReceipt(buildReceipt(QStringLiteral("maker"),
                                parseObject(resultJson), m_makerEvidence));
    m_makerEvidence = QJsonObject{};

    setMakerResultJson(resultJson);
    if (!isErrorResult(resultJson)) {
        addMakerProgressStep(QStringLiteral("EthClaimed"));
        setMakerCurrentStep(QStringLiteral("EthClaimed"));
    }
    setMakerRunning(false);
    setMakerJobId(QString{});
    updateRunning();
    if (m_coordinationRole == QStringLiteral("maker")) {
        coordinationStop();
    }
    setResultStatus(resultJson,
                    QStringLiteral("Maker swap finished"),
                    QStringLiteral("Maker swap failed"));
    requestAutomaticBalanceRefresh();
}

void SwapUiPlugin::handleTakerFinished(const QString& resultJson)
{
    // Journal before any state resets (see handleMakerFinished).
    journalReceipt(buildReceipt(QStringLiteral("taker"),
                                parseObject(resultJson), m_takerEvidence));
    m_takerEvidence = QJsonObject{};

    setTakerResultJson(resultJson);
    if (!isErrorResult(resultJson)) {
        addTakerProgressStep(QStringLiteral("LezClaimed"));
        setTakerCurrentStep(QStringLiteral("LezClaimed"));
    }
    setTakerRunning(false);
    setTakerJobId(QString{});
    updateRunning();
    if (m_coordinationRole == QStringLiteral("taker")) {
        coordinationStop();
    }
    setResultStatus(resultJson,
                    QStringLiteral("Taker swap finished"),
                    QStringLiteral("Taker swap failed"));
    requestAutomaticBalanceRefresh();
}

void SwapUiPlugin::handleAutoAcceptFinished(const QString& resultJson)
{
    const auto obj = parseObject(resultJson);
    if (!isErrorResult(resultJson)) {
        const auto completed = obj.value(QStringLiteral("completed")).toInt(autoAcceptCompleted());
        const auto failed = obj.value(QStringLiteral("failed")).toInt(autoAcceptFailed());
        setAutoAcceptCompleted(completed);
        setAutoAcceptFailed(failed);
    }

    setAutoAcceptRunning(false);
    setMakerRunning(false);
    setAutoAcceptJobId(QString{});
    setMakerJobId(QString{});
    updateRunning();
    if (m_coordinationRole == QStringLiteral("maker")) {
        coordinationStop();
    }
    setResultStatus(resultJson,
                    QStringLiteral("Auto-accept stopped"),
                    QStringLiteral("Auto-accept failed"));
    requestAutomaticBalanceRefresh();
}

void SwapUiPlugin::handleSetupFundingFinished(const QString& resultJson)
{
    const auto error = jsonError(resultJson);
    setSetupRunning(false);
    setSetupJobId(QString{});
    if (!error.isEmpty()) {
        setSetupError(error);
        setStatus(QStringLiteral("LEZ account setup failed: %1").arg(error));
        return;
    }

    const auto obj = parseObject(resultJson);
    if (obj.contains(QStringLiteral("balance"))) {
        setSetupBalance(valueString(obj, QStringLiteral("balance")));
    }
    setSetupError(QString{});
    setSetupStep(QStringLiteral("Done"));
    setStatus(QStringLiteral("LEZ account ready"));
    // Refresh the header's ethAddress/lezAccount/lezBalance display now that
    // the account actually has funds — SetupView's own PROPs above only
    // cover the in-progress job, not the post-funding balance readout the
    // rest of the app already shows.
    fetchBalances();
}

void SwapUiPlugin::handleJobStartResult(const QString& role, const QString& resultJson)
{
    const auto error = jsonError(resultJson);
    if (!error.isEmpty()) {
        setErrorMessage(error);
        setStatus(QStringLiteral("Failed to start %1: %2").arg(role, error));
        if (role == QStringLiteral("maker")) {
            setMakerRunning(false);
            setMakerJobId(QString{});
        } else if (role == QStringLiteral("taker")) {
            setTakerRunning(false);
            setTakerJobId(QString{});
        } else if (role == QStringLiteral("maker_loop")) {
            setAutoAcceptRunning(false);
            setMakerRunning(false);
            setAutoAcceptJobId(QString{});
            setMakerJobId(QString{});
        }
        updateRunning();
        return;
    }

    const auto jobId = jobIdFromResult(resultJson);
    swapUiTrace(QStringLiteral("handleJobStartResult role=%1 jobId=%2 result.bytes=%3")
                    .arg(role, jobId, QString::number(resultJson.size())));

    // Late start-ack guard: if a fast *.finished event has already
    // cleared the running flag for this role, the orchestrator job is
    // already terminal. Don't resurrect it by setting the job_id and
    // marking it running again.
    if (role == QStringLiteral("maker")) {
        if (!makerRunning()) {
            swapUiTrace(QStringLiteral("handleJobStartResult role=maker dropped — role no longer running (finished before ack)"));
            return;
        }
        setMakerJobId(jobId);
        setStatus(QStringLiteral("Maker swap running"));
    } else if (role == QStringLiteral("taker")) {
        if (!takerRunning()) {
            swapUiTrace(QStringLiteral("handleJobStartResult role=taker dropped — role no longer running (finished before ack)"));
            return;
        }
        setTakerJobId(jobId);
        setStatus(QStringLiteral("Taker swap running"));
    } else if (role == QStringLiteral("maker_loop")) {
        if (!autoAcceptRunning()) {
            swapUiTrace(QStringLiteral("handleJobStartResult role=maker_loop dropped — role no longer running (finished before ack)"));
            return;
        }
        setAutoAcceptJobId(jobId);
        setMakerJobId(jobId);
        setStatus(QStringLiteral("Live maker listener running"));
    }
    updateRunning();
}

void SwapUiPlugin::startMaker(const QString& hashlockHex)
{
    if (!m_swap || makerRunning() || autoAcceptRunning() || takerRunning()) {
        return;
    }
    if (!validateConfigForAction(QStringLiteral("maker"), hashlockHex, QStringLiteral("hashlock_hex"))) {
        return;
    }
    if (!subscribeToSwapEvents()) {
        setErrorMessage(QStringLiteral("Cannot start maker: swap event subscription unavailable"));
        setStatus(errorMessage());
        return;
    }

    setMakerRunning(true);
    setMakerJobId(QString{});
    clearMakerProgress();
    beginMakerEvidence(true);
    setMakerCurrentStep(QStringLiteral("WaitingForEthLock"));
    addMakerProgressStep(QStringLiteral("WaitingForEthLock"));
    setStatus(QStringLiteral("Starting maker swap..."));
    setBusyState();

    m_swap->startMakerJobAsync(configJson(), hashlockHex, [this](QString result) {
        handleJobStartResult(QStringLiteral("maker"), result);
    });
}

void SwapUiPlugin::startTaker(const QString& preimageHex)
{
    // Consume the accepted-offer context up front (set by
    // acceptOfferAndStartTaker just before this call) so a rejected start
    // can never leak a stale offer into a later manual run.
    const QJsonObject offerContext = m_pendingOfferContext;
    m_pendingOfferContext = QJsonObject{};

    if (!m_swap || takerRunning() || makerRunning() || autoAcceptRunning()) {
        return;
    }
    if (!validateConfigForAction(QStringLiteral("taker"), preimageHex, QStringLiteral("preimage_hex"))) {
        return;
    }
    if (!subscribeToSwapEvents()) {
        setErrorMessage(QStringLiteral("Cannot start taker: swap event subscription unavailable"));
        setStatus(errorMessage());
        return;
    }

    setTakerRunning(true);
    setTakerJobId(QString{});
    clearTakerProgress();
    beginTakerEvidence(offerContext);
    setTakerCurrentStep(QStringLiteral("PreimageGenerated"));
    addTakerProgressStep(QStringLiteral("PreimageGenerated"));
    setStatus(QStringLiteral("Starting taker swap..."));
    setBusyState();

    m_swap->startTakerJobAsync(configJson(), preimageHex, [this](QString result) {
        handleJobStartResult(QStringLiteral("taker"), result);
    });
}

void SwapUiPlugin::acceptOfferAndStartTaker(const QString& offerJson)
{
    const auto offer = parseObject(offerJson);
    if (offer.isEmpty()) {
        setErrorMessage(QStringLiteral("Invalid offer JSON"));
        setStatus(errorMessage());
        return;
    }
    // P0: applyOfferObject refuses (and mutates nothing) if the offer names a
    // non-canonical swap venue. Never start the taker in that case — do not
    // clear the pending context, do not touch config.
    if (!applyOfferObject(offer)) {
        return;
    }
    // Hand the full offer to startTaker's evidence capture: it carries the
    // exact wei amount, the maker's LEZ account and the absolute timelocks,
    // none of which survive applyOfferObject's config-field mapping.
    m_pendingOfferContext = offer;
    startTaker(QString{});
}

void SwapUiPlugin::refundLez(const QString& hashlockHex)
{
    if (!m_swap || makerRunning() || takerRunning() || autoAcceptRunning() || refundsLoading()) {
        return;
    }
    if (!validateConfigForAction(QStringLiteral("refund_lez"), hashlockHex, QStringLiteral("refund_lez_hashlock"))) {
        return;
    }

    setMakerRunning(true);
    setRefundsLoading(true);
    clearMakerProgress();
    // Minimal evidence for a manual recovery: only what is actually known
    // about the refunded swap. Config amounts/counterparty describe the
    // *current* rate, not the (possibly older) swap being refunded, so a
    // truthful receipt omits them.
    m_makerEvidence = QJsonObject{};
    m_makerEvidence.insert(QStringLiteral("started_ms"),
                           static_cast<double>(QDateTime::currentMSecsSinceEpoch()));
    m_makerEvidence.insert(QStringLiteral("hashlock"), normaliseHashlock(hashlockHex));
    m_makerEvidence.insert(QStringLiteral("lez_program_id"), lezHtlcProgramId());
    m_makerEvidence.insert(QStringLiteral("lez_sequencer"), lezSequencerUrl());
    m_makerEvidence.insert(QStringLiteral("eth_rpc"), ethRpcUrl());
    setMakerCurrentStep(QStringLiteral("Refunding"));
    addMakerProgressStep(QStringLiteral("Refunding"));
    setStatus(QStringLiteral("Refunding LEZ..."));
    setBusyState();

    m_swap->refundLezAsync(configJson(), hashlockHex, [this](QString result) {
        // Journal successful manual refunds only: failed attempts are
        // usually premature retries against an unexpired timelock, not swap
        // outcomes, and would spam the history.
        if (!isErrorResult(result)) {
            QJsonObject refundResult;
            refundResult.insert(QStringLiteral("status"), QStringLiteral("refunded"));
            refundResult.insert(QStringLiteral("lez_refund_tx"),
                                valueString(parseObject(result), QStringLiteral("tx_hash")));
            journalReceipt(buildReceipt(QStringLiteral("maker"),
                                        refundResult, m_makerEvidence));
        }
        m_makerEvidence = QJsonObject{};
        setMakerResultJson(result);
        setMakerRunning(false);
        setRefundsLoading(false);
        updateRunning();
        setResultStatus(result,
                        QStringLiteral("LEZ refund finished"),
                        QStringLiteral("LEZ refund failed"));
        fetchBalancesFromLoadedEnv();
    });
}

void SwapUiPlugin::refundEth(const QString& swapIdHex)
{
    if (!m_swap || takerRunning() || makerRunning() || autoAcceptRunning() || refundsLoading()) {
        return;
    }
    if (!validateConfigForAction(QStringLiteral("refund_eth"), swapIdHex, QStringLiteral("refund_eth_swap_id"))) {
        return;
    }

    setTakerRunning(true);
    setRefundsLoading(true);
    clearTakerProgress();
    // Minimal evidence for a manual recovery — see refundLez.
    m_takerEvidence = QJsonObject{};
    m_takerEvidence.insert(QStringLiteral("started_ms"),
                           static_cast<double>(QDateTime::currentMSecsSinceEpoch()));
    m_takerEvidence.insert(QStringLiteral("eth_swap_id"), swapIdHex.trimmed());
    m_takerEvidence.insert(QStringLiteral("eth_htlc_address"), ethHtlcAddress());
    m_takerEvidence.insert(QStringLiteral("lez_sequencer"), lezSequencerUrl());
    m_takerEvidence.insert(QStringLiteral("eth_rpc"), ethRpcUrl());
    setTakerCurrentStep(QStringLiteral("Refunding"));
    addTakerProgressStep(QStringLiteral("Refunding"));
    setStatus(QStringLiteral("Refunding ETH..."));
    setBusyState();

    m_swap->refundEthAsync(configJson(), swapIdHex, [this](QString result) {
        // Successful manual refunds only — see refundLez.
        if (!isErrorResult(result)) {
            QJsonObject refundResult;
            refundResult.insert(QStringLiteral("status"), QStringLiteral("refunded"));
            refundResult.insert(QStringLiteral("eth_refund_tx"),
                                valueString(parseObject(result), QStringLiteral("tx_hash")));
            journalReceipt(buildReceipt(QStringLiteral("taker"),
                                        refundResult, m_takerEvidence));
        }
        m_takerEvidence = QJsonObject{};
        setTakerResultJson(result);
        setTakerRunning(false);
        setRefundsLoading(false);
        updateRunning();
        setResultStatus(result,
                        QStringLiteral("ETH refund finished"),
                        QStringLiteral("ETH refund failed"));
        fetchBalancesFromLoadedEnv();
    });
}

void SwapUiPlugin::startBackgroundServices()
{
    if (!m_swap) {
        return;
    }
    m_autoMessagingEnabled = true;
    ensureMessagingReady({}, true);
}

void SwapUiPlugin::ensureMessagingReady(std::function<void()> continuation, bool automatic)
{
    if (!m_swap) {
        setStatus(QStringLiteral("Swap client not ready"));
        return;
    }
    if (messagingConnected()) {
        setMessagingRetrying(false);
        if (continuation) {
            continuation();
        }
        return;
    }
    if (m_messagingInitInFlight) {
        if (continuation) {
            m_pendingMessagingContinuations.push_back(std::move(continuation));
        }
        setStatus(QStringLiteral("Messaging is connecting..."));
        return;
    }

    if (continuation) {
        m_pendingMessagingContinuations.push_back(std::move(continuation));
    }

    m_messagingInitInFlight = true;
    setMessagingLoading(true);
    setMessagingRetrying(automatic || m_autoMessagingEnabled);
    setStatus(automatic
        ? QStringLiteral("Starting Delivery messaging...")
        : QStringLiteral("Connecting to messaging..."));
    m_swap->messagingInitAsync(messagingConfigJson(), [this](QString result) {
        m_messagingInitInFlight = false;
        setMessagingLoading(false);
        const auto error = jsonError(result);
        if (!error.isEmpty()) {
            setErrorMessage(error);
            if (m_autoMessagingEnabled) {
                setMessagingRetrying(true);
                setStatus(QStringLiteral("Delivery unavailable; retrying automatically: %1").arg(error));
            } else {
                setMessagingRetrying(false);
                setStatus(QStringLiteral("Messaging connect failed: %1").arg(error));
            }
            m_pendingMessagingContinuations.clear();
            return;
        }
        setErrorMessage(QString{});
        setMessagingRetrying(false);
        setStatus(QStringLiteral("Messaging connected"));
        pollMessagingStatus();
        auto continuations = std::move(m_pendingMessagingContinuations);
        m_pendingMessagingContinuations.clear();
        for (auto& pending : continuations) {
            if (pending) {
                pending();
            }
        }
    });
}

void SwapUiPlugin::initMessaging()
{
    ensureMessagingReady();
}

void SwapUiPlugin::pollMessagingStatus()
{
    if (!m_swap) {
        return;
    }
    m_swap->messagingStatusAsync([this](QString result) {
        if (isErrorResult(result)) {
            if (m_autoMessagingEnabled && !m_messagingInitInFlight) {
                ensureMessagingReady({}, true);
            }
            return;
        }
        const auto obj = parseObject(result);
        const bool connected = obj.value(QStringLiteral("connected")).toBool(false);
        setMessagingConnected(connected);
        setMessagingPeerCount(obj.value(QStringLiteral("peer_count")).toInt(0));
        setMessagingPeerCountKnown(
            obj.value(QStringLiteral("peer_count_known")).toBool(false));
        setMessagingConnectionStatus(obj.value(QStringLiteral("connection_status")).toString());
        setMessagingHint(obj.value(QStringLiteral("delivery_hint")).toString());
        if (connected) {
            setMessagingRetrying(false);
        } else if (m_autoMessagingEnabled && !m_messagingInitInFlight) {
            ensureMessagingReady({}, true);
        }
    });
}

bool SwapUiPlugin::subscribeToSwapEvents()
{
    if (m_swapEventsSubscribed && m_eventObject) {
        return true;
    }
    if (!m_logosAPI) {
        swapUiTrace(QStringLiteral("subscribeToSwapEvents: m_logosAPI null"));
        return false;
    }
    LogosAPIClient* client = m_logosAPI->getClient(kSwapModuleName);
    if (!client) {
        swapUiTrace(QStringLiteral("subscribeToSwapEvents: getClient(swap) returned null"));
        setErrorMessage(QStringLiteral("swap event client unavailable"));
        return false;
    }

    m_eventObject = client->requestObject(QString::fromUtf8(kSwapModuleName));
    if (!m_eventObject) {
        swapUiTrace(QStringLiteral("subscribeToSwapEvents: requestObject(swap) returned null"));
        setErrorMessage(QStringLiteral("swap event object unavailable"));
        return false;
    }

    const QStringList eventNames{
        QStringLiteral("maker.progress"),
        QStringLiteral("taker.progress"),
        QStringLiteral("maker_loop.progress"),
        QStringLiteral("lez_setup.progress"),
        QStringLiteral("maker.finished"),
        QStringLiteral("taker.finished"),
        QStringLiteral("maker_loop.finished"),
        QStringLiteral("lez_setup.finished")
    };

    for (const QString& eventName : eventNames) {
        client->onEvent(m_eventObject, eventName, [this](const QString& name, const QVariantList& args) {
            onSwapEventArgs(name, args);
        });
    }

    m_swapEventsSubscribed = true;
    swapUiTrace(QStringLiteral("subscribeToSwapEvents: subscribed to %1 events").arg(eventNames.size()));
    return true;
}

void SwapUiPlugin::onSwapEventArgs(const QString& eventName, const QVariantList& args)
{
    QStringList typeNames;
    typeNames.reserve(args.size());
    for (const QVariant& v : args) {
        typeNames.append(QString::fromUtf8(v.typeName() ? v.typeName() : "<null>"));
    }
    swapUiTrace(QStringLiteral("onSwapEventArgs event=%1 args.size=%2 types=[%3]")
                    .arg(eventName, QString::number(args.size()), typeNames.join(QStringLiteral(","))));
    onSwapEvent(eventName, payloadFromArgs(args));
}

void SwapUiPlugin::onSwapEvent(const QString& eventName, const QString& payloadJson)
{
    swapUiTrace(QStringLiteral("RX event=%1 payload.bytes=%2 payload.head=%3")
                    .arg(eventName, QString::number(payloadJson.size()), payloadJson.left(160)));
    const auto payload = parseObject(payloadJson);
    if (payload.isEmpty()) {
        swapUiTrace(QStringLiteral("RX event=%1 DROP reason=empty_or_unparseable_payload").arg(eventName));
        return;
    }
    if (eventName.endsWith(QStringLiteral(".progress"))) {
        handleProgressEvent(eventName, payload);
    } else if (eventName.endsWith(QStringLiteral(".finished"))) {
        handleFinishedEvent(eventName, payload);
    } else {
        swapUiTrace(QStringLiteral("RX event=%1 DROP reason=unknown_event_suffix").arg(eventName));
    }
}

void SwapUiPlugin::handleProgressEvent(const QString& eventName, const QJsonObject& payload)
{
    const auto step = valueString(payload, QStringLiteral("step"));
    const auto data = payload.value(QStringLiteral("data")).toObject();
    const auto jobId = valueString(payload, QStringLiteral("job_id"));

    if (eventName == QStringLiteral("maker.progress")) {
        if (!shouldHandleJobEvent(eventName, payload)) {
            return;
        }
        setMakerCurrentStep(step);
        addMakerProgressStep(step);
        captureMakerProgressEvidence(step, data, payload);
        if (step == QStringLiteral("EthLockDetected")) {
            const auto hashlock = normaliseHashlock(
                valueString(data, QStringLiteral("hashlock")));
            if (!hashlock.isEmpty()) {
                coordinationStart(QStringLiteral("maker"), hashlock);
            }
        }
        return;
    }

    if (eventName == QStringLiteral("taker.progress")) {
        if (!shouldHandleJobEvent(eventName, payload)) {
            return;
        }
        setTakerCurrentStep(step);
        addTakerProgressStep(step);
        captureTakerProgressEvidence(step, data, payload);
        if (step == QStringLiteral("PreimageGenerated")) {
            const auto hashlock = normaliseHashlock(
                valueString(data, QStringLiteral("hashlock")));
            if (!hashlock.isEmpty()) {
                coordinationStart(QStringLiteral("taker"), hashlock);
            }
        } else if (step == QStringLiteral("EthLocked")) {
            const auto swapId = valueString(data, QStringLiteral("swap_id"));
            coordinationPublishTakerAccept(coordinationActiveHashlock(), swapId);
        }
        return;
    }

    if (eventName == QStringLiteral("lez_setup.progress")) {
        if (!shouldHandleJobEvent(eventName, payload)) {
            return;
        }
        // Steps mirror src/lez/onboard.rs::FundingProgress: Initializing,
        // AlreadyInitialized, Initialized, CheckingBalance, ClaimSubmitted,
        // Claimed, ClaimFailed, TargetReached. Surfacing the raw step name
        // (rather than translating it) keeps SetupView.qml's progress text in
        // sync with the Rust doc comments describing each phase. Claimed now
        // fires only once a claim COMMITS on-chain and carries the balance
        // from the same commit read, so the claims counter and the balance
        // readout can never contradict each other.
        setSetupStep(step);
        if (data.contains(QStringLiteral("balance"))) {
            setSetupBalance(valueString(data, QStringLiteral("balance")));
        }
        if (step == QStringLiteral("Claimed")) {
            setSetupClaims(data.value(QStringLiteral("total_claims")).toInt(setupClaims() + 1));
        }
        return;
    }

    if (eventName != QStringLiteral("maker_loop.progress")) {
        return;
    }
    if (!shouldHandleJobEvent(eventName, payload)) {
        return;
    }

    if (step == QStringLiteral("AutoAcceptIteration")) {
        setAutoAcceptIteration(data.value(QStringLiteral("iteration")).toInt(autoAcceptIteration() + 1));
        setMakerCurrentStep(QStringLiteral("WaitingForEthLock"));
        addMakerProgressStep(QStringLiteral("WaitingForEthLock"));
        // Each iteration starts a fresh swap with a fresh hashlock, so the
        // previous per-swap Delivery subscription (if any) is no longer
        // useful. Unsubscribe so the maker doesn't keep accumulating
        // /atomic-swaps/1/swap-<hashlock>/json subscriptions across the
        // life of the auto-accept loop.
        if (m_coordinationRole == QStringLiteral("maker")) {
            coordinationStop();
        }
        // Fresh iteration, fresh evidence: any per-swap data still held
        // belongs to the previous iteration's swap and was journaled at its
        // completion event.
        beginMakerEvidence(false);
    } else if (step == QStringLiteral("AutoAcceptSwapCompleted")) {
        setAutoAcceptCompleted(autoAcceptCompleted() + 1);
        QJsonObject entry;
        entry.insert(QStringLiteral("status"), QStringLiteral("completed"));
        entry.insert(QStringLiteral("lez_amount"), lezAmount());
        entry.insert(QStringLiteral("eth_amount"), ethAmount());
        entry.insert(QStringLiteral("timestamp"), QDateTime::currentMSecsSinceEpoch());
        entry.insert(QStringLiteral("iteration"), data.value(QStringLiteral("iteration")).toInt(autoAcceptIteration()));
        auto history = swapHistory();
        history.prepend(compactJson(entry));
        setSwapHistory(history);
        // Journal the per-swap completion. AutoAcceptSwapCompleted itself
        // carries only {iteration, status} (outcome enrichment is PR3), but
        // the loop forwards the per-swap progress steps through
        // maker_loop.progress, so the accumulated evidence already holds
        // hashlock, LEZ lock tx, preimage and ETH claim tx.
        {
            QJsonObject result;
            result.insert(QStringLiteral("status"), QStringLiteral("completed"));
            QJsonObject receipt = buildReceipt(QStringLiteral("maker"),
                                               result, m_makerEvidence);
            receipt.insert(QStringLiteral("iteration"),
                           data.value(QStringLiteral("iteration")).toInt(autoAcceptIteration()));
            journalReceipt(receipt);
            beginMakerEvidence(false);
        }
        clearMakerProgress();
        setMakerCurrentStep(QStringLiteral("WaitingForEthLock"));
        requestAutomaticBalanceRefresh();
    } else if (step == QStringLiteral("AutoAcceptSwapFailed")) {
        setAutoAcceptFailed(autoAcceptFailed() + 1);
        QJsonObject entry;
        entry.insert(QStringLiteral("status"), QStringLiteral("failed"));
        entry.insert(QStringLiteral("error"), valueString(data, QStringLiteral("error")));
        entry.insert(QStringLiteral("timestamp"), QDateTime::currentMSecsSinceEpoch());
        entry.insert(QStringLiteral("iteration"), data.value(QStringLiteral("iteration")).toInt(autoAcceptIteration()));
        auto history = swapHistory();
        history.prepend(compactJson(entry));
        setSwapHistory(history);
        // Journal the per-swap failure — the history is a truthful log.
        {
            QJsonObject result;
            result.insert(QStringLiteral("status"), QStringLiteral("failed"));
            result.insert(QStringLiteral("error"),
                          valueString(data, QStringLiteral("error")));
            QJsonObject receipt = buildReceipt(QStringLiteral("maker"),
                                               result, m_makerEvidence);
            receipt.insert(QStringLiteral("iteration"),
                           data.value(QStringLiteral("iteration")).toInt(autoAcceptIteration()));
            journalReceipt(receipt);
            beginMakerEvidence(false);
        }
        clearMakerProgress();
        setMakerCurrentStep(QStringLiteral("WaitingForEthLock"));
    } else if (step == QStringLiteral("AutoAcceptInsufficientFunds")) {
        QJsonObject entry;
        entry.insert(QStringLiteral("status"), QStringLiteral("insufficient_funds"));
        entry.insert(QStringLiteral("lez_balance"), valueString(data, QStringLiteral("lez_balance")));
        entry.insert(QStringLiteral("lez_required"), valueString(data, QStringLiteral("lez_required")));
        entry.insert(QStringLiteral("timestamp"), QDateTime::currentMSecsSinceEpoch());
        auto history = swapHistory();
        history.prepend(compactJson(entry));
        setSwapHistory(history);
    } else {
        setMakerCurrentStep(step);
        addMakerProgressStep(step);
        // The loop forwards the per-swap maker steps through
        // maker_loop.progress — capture their evidence exactly like the
        // single-shot path so loop receipts carry tx hashes too.
        captureMakerProgressEvidence(step, data, payload);
        // Mirror the single-shot maker.progress hook: the auto-accept loop
        // forwards the per-swap EthLockDetected through maker_loop.progress,
        // and without subscribing here the board-path maker never joins the
        // per-swap coordination topic — the taker's SwapAccept reaches the
        // maker's waku node and is dropped ("no subscribed peers found"), so
        // coordination events never show on the board path.
        if (step == QStringLiteral("EthLockDetected")) {
            const auto hashlock = normaliseHashlock(
                valueString(data, QStringLiteral("hashlock")));
            if (!hashlock.isEmpty()) {
                coordinationStart(QStringLiteral("maker"), hashlock);
            }
        }
    }
}

void SwapUiPlugin::handleFinishedEvent(const QString& eventName, const QJsonObject& payload)
{
    if (!shouldHandleJobEvent(eventName, payload)) {
        return;
    }
    auto resultJson = compactJsonValue(payload.value(QStringLiteral("result")));
    const auto error = valueString(payload, QStringLiteral("error"));
    if (!error.isEmpty() && jsonError(resultJson).isEmpty()) {
        QJsonObject resultObj = parseObject(resultJson);
        resultObj.insert(QStringLiteral("error"), error);
        resultJson = compactJson(resultObj);
    }

    if (eventName == QStringLiteral("maker.finished")) {
        handleMakerFinished(resultJson);
    } else if (eventName == QStringLiteral("taker.finished")) {
        handleTakerFinished(resultJson);
    } else if (eventName == QStringLiteral("maker_loop.finished")) {
        handleAutoAcceptFinished(resultJson);
    } else if (eventName == QStringLiteral("lez_setup.finished")) {
        handleSetupFundingFinished(resultJson);
    }
}

// ---------------------------------------------------------------------------
// Receipt capture + durable JSONL journal (receipt cards, PR2)
// ---------------------------------------------------------------------------

qint64 SwapUiPlugin::eventTimestampMs(const QJsonObject& payload)
{
    const auto ts = static_cast<qint64>(
        payload.value(QStringLiteral("timestamp_ms")).toDouble(0));
    return ts > 0 ? ts : QDateTime::currentMSecsSinceEpoch();
}

// Config values every receipt wants regardless of role. Snapshotted at run
// start so a mid-swap config edit cannot corrupt the receipt.
void SwapUiPlugin::snapshotEvidenceFromConfig(QJsonObject& evidence) const
{
    evidence.insert(QStringLiteral("lez_amount"), lezAmount());
    evidence.insert(QStringLiteral("eth_amount"), ethAmount());
    evidence.insert(QStringLiteral("eth_htlc_address"), ethHtlcAddress());
    evidence.insert(QStringLiteral("lez_program_id"), lezHtlcProgramId());
    evidence.insert(QStringLiteral("lez_timelock_minutes"), lezTimelockMinutes());
    evidence.insert(QStringLiteral("eth_timelock_minutes"), ethTimelockMinutes());
    evidence.insert(QStringLiteral("lez_sequencer"), lezSequencerUrl());
    evidence.insert(QStringLiteral("eth_rpc"), ethRpcUrl());
}

// Taker evidence starts at startTaker/refundEth. After applyOfferObject the
// recipient field holds the maker's ETH address, so it doubles as the
// counterparty for both the board flow and a manually configured run. The
// offer context (when the run came from an accepted offer) adds the exact
// wei amount, the maker's LEZ account and the absolute timelocks.
void SwapUiPlugin::beginTakerEvidence(const QJsonObject& offerContext)
{
    m_takerEvidence = QJsonObject{};
    snapshotEvidenceFromConfig(m_takerEvidence);
    m_takerEvidence.insert(QStringLiteral("started_ms"),
                           QDateTime::currentMSecsSinceEpoch());
    m_takerEvidence.insert(QStringLiteral("counterparty_eth"), ethRecipientAddress());
    if (offerContext.isEmpty()) {
        return;
    }
    m_takerEvidence.insert(QStringLiteral("eth_amount_wei"),
                           valueString(offerContext, QStringLiteral("eth_amount")));
    m_takerEvidence.insert(QStringLiteral("counterparty_lez"),
                           valueString(offerContext, QStringLiteral("maker_lez_account")));
    const auto lezUnix = offerContext.value(QStringLiteral("lez_timelock")).toDouble(0);
    const auto ethUnix = offerContext.value(QStringLiteral("eth_timelock")).toDouble(0);
    if (lezUnix > 0) {
        m_takerEvidence.insert(QStringLiteral("lez_timelock_unix"), lezUnix);
    }
    if (ethUnix > 0) {
        m_takerEvidence.insert(QStringLiteral("eth_timelock_unix"), ethUnix);
    }
}

// Maker evidence starts at startMaker/refundLez/startAutoAccept and resets
// on every AutoAcceptIteration. Loop iterations pass stampStart=false: the
// swap's wall clock starts when a buyer engages (EthLockDetected), not when
// the listener goes live.
void SwapUiPlugin::beginMakerEvidence(bool stampStart)
{
    m_makerEvidence = QJsonObject{};
    snapshotEvidenceFromConfig(m_makerEvidence);
    if (stampStart) {
        m_makerEvidence.insert(QStringLiteral("started_ms"),
                               QDateTime::currentMSecsSinceEpoch());
    }
    // Config's preconfigured taker recipient; overwritten by the Delivery
    // SwapAccept when board-path coordination supplies the real identity.
    m_makerEvidence.insert(QStringLiteral("counterparty_lez"), lezTakerAccountId());
}

void SwapUiPlugin::captureTakerProgressEvidence(const QString& step,
                                                const QJsonObject& data,
                                                const QJsonObject& payload)
{
    Q_UNUSED(payload);
    if (step == QStringLiteral("PreimageGenerated")) {
        m_takerEvidence.insert(QStringLiteral("hashlock"),
                               valueString(data, QStringLiteral("hashlock")));
    } else if (step == QStringLiteral("EthLocked")) {
        m_takerEvidence.insert(QStringLiteral("eth_swap_id"),
                               valueString(data, QStringLiteral("swap_id")));
        m_takerEvidence.insert(QStringLiteral("eth_lock_tx"),
                               valueString(data, QStringLiteral("tx_hash")));
        m_takerEvidence.insert(QStringLiteral("eth_chain_id"),
                               data.value(QStringLiteral("chain_id")).toDouble(0));
    } else if (step == QStringLiteral("LezClaimed")) {
        m_takerEvidence.insert(QStringLiteral("lez_claim_tx"),
                               valueString(data, QStringLiteral("tx_hash")));
    }
}

void SwapUiPlugin::captureMakerProgressEvidence(const QString& step,
                                                const QJsonObject& data,
                                                const QJsonObject& payload)
{
    if (step == QStringLiteral("EthLockDetected")) {
        if (!m_makerEvidence.contains(QStringLiteral("started_ms"))) {
            m_makerEvidence.insert(QStringLiteral("started_ms"),
                                   static_cast<double>(eventTimestampMs(payload)));
        }
        m_makerEvidence.insert(QStringLiteral("eth_swap_id"),
                               valueString(data, QStringLiteral("swap_id")));
        m_makerEvidence.insert(QStringLiteral("hashlock"),
                               valueString(data, QStringLiteral("hashlock")));
        m_makerEvidence.insert(QStringLiteral("eth_lock_tx"),
                               valueString(data, QStringLiteral("tx_hash")));
        m_makerEvidence.insert(QStringLiteral("eth_chain_id"),
                               data.value(QStringLiteral("chain_id")).toDouble(0));
    } else if (step == QStringLiteral("LezLocked")) {
        m_makerEvidence.insert(QStringLiteral("lez_lock_tx"),
                               valueString(data, QStringLiteral("tx_hash")));
    } else if (step == QStringLiteral("PreimageRevealed")) {
        m_makerEvidence.insert(QStringLiteral("preimage"),
                               valueString(data, QStringLiteral("preimage")));
    } else if (step == QStringLiteral("EthClaimed")) {
        m_makerEvidence.insert(QStringLiteral("eth_claim_tx"),
                               valueString(data, QStringLiteral("tx_hash")));
    }
}

// Assemble a swap-receipt/1 object (dossier §3b; field-compatible with the
// PR1 copy-JSON receipt, extended additively with `network`). The result
// JSON is authoritative where present; accumulated evidence fills the rest.
// The role-dependent `eth_tx`/`lez_tx` semantics from src/swap/types.rs are
// resolved here once, so consumers never see the mislabel: eth_tx is the
// maker's ETH *claim tx* but the taker's ETH lock *swap id*; lez_tx is the
// maker's LEZ *lock tx* but the taker's LEZ *claim tx*.
QJsonObject SwapUiPlugin::buildReceipt(const QString& role,
                                       const QJsonObject& result,
                                       const QJsonObject& evidence) const
{
    const auto str = [](const QJsonObject& obj, const char* key) {
        return valueString(obj, QString::fromLatin1(key));
    };
    const auto pick = [](const QString& a, const QString& b) {
        return !a.isEmpty() ? a : b;
    };
    const auto jstr = [](const QString& s) {
        return s.isEmpty() ? QJsonValue(QJsonValue::Null) : QJsonValue(s);
    };
    const auto jnum = [&evidence](const char* key) {
        const auto v = evidence.value(QString::fromLatin1(key)).toDouble(0);
        return v > 0 ? QJsonValue(v) : QJsonValue(QJsonValue::Null);
    };

    const bool maker = role == QStringLiteral("maker");
    const QString error = str(result, "error");
    QString status = str(result, "status");
    if (status.isEmpty()) {
        status = error.isEmpty() ? QStringLiteral("unknown") : QStringLiteral("failed");
    }

    const QString resultEthTx = str(result, "eth_tx");
    const QString resultLezTx = str(result, "lez_tx");

    QJsonObject eth;
    eth.insert(QStringLiteral("swap_id"),
               jstr(maker ? str(evidence, "eth_swap_id")
                          : pick(resultEthTx, str(evidence, "eth_swap_id"))));
    eth.insert(QStringLiteral("lock_tx"), jstr(str(evidence, "eth_lock_tx")));
    eth.insert(QStringLiteral("claim_tx"),
               jstr(maker ? pick(resultEthTx, str(evidence, "eth_claim_tx"))
                          : QString{}));
    eth.insert(QStringLiteral("refund_tx"), jstr(str(result, "eth_refund_tx")));
    eth.insert(QStringLiteral("htlc_address"), jstr(str(evidence, "eth_htlc_address")));
    eth.insert(QStringLiteral("counterparty"), jstr(str(evidence, "counterparty_eth")));

    QJsonObject lez;
    lez.insert(QStringLiteral("lock_tx"),
               jstr(maker ? pick(resultLezTx, str(evidence, "lez_lock_tx"))
                          : QString{}));
    lez.insert(QStringLiteral("claim_tx"),
               jstr(maker ? QString{}
                          : pick(resultLezTx, str(evidence, "lez_claim_tx"))));
    lez.insert(QStringLiteral("refund_tx"), jstr(str(result, "lez_refund_tx")));
    lez.insert(QStringLiteral("program_id"), jstr(str(evidence, "lez_program_id")));
    lez.insert(QStringLiteral("counterparty"), jstr(str(evidence, "counterparty_lez")));

    QJsonObject timelocks;
    timelocks.insert(QStringLiteral("lez_unix"), jnum("lez_timelock_unix"));
    timelocks.insert(QStringLiteral("eth_unix"), jnum("eth_timelock_unix"));
    timelocks.insert(QStringLiteral("lez_minutes"), jstr(str(evidence, "lez_timelock_minutes")));
    timelocks.insert(QStringLiteral("eth_minutes"), jstr(str(evidence, "eth_timelock_minutes")));

    QJsonObject receipt;
    receipt.insert(QStringLiteral("schema"), QStringLiteral("swap-receipt/1"));
    receipt.insert(QStringLiteral("role"), role);
    receipt.insert(QStringLiteral("status"), status);
    receipt.insert(QStringLiteral("hashlock"),
                   jstr(pick(str(result, "hashlock"), str(evidence, "hashlock"))));
    receipt.insert(QStringLiteral("preimage"),
                   jstr(pick(str(result, "preimage"), str(evidence, "preimage"))));
    receipt.insert(QStringLiteral("lez_amount"), jstr(str(evidence, "lez_amount")));
    receipt.insert(QStringLiteral("eth_amount"), jstr(str(evidence, "eth_amount")));
    receipt.insert(QStringLiteral("eth_amount_wei"), jstr(str(evidence, "eth_amount_wei")));
    receipt.insert(QStringLiteral("eth"), eth);
    receipt.insert(QStringLiteral("lez"), lez);
    receipt.insert(QStringLiteral("timelocks"), timelocks);
    receipt.insert(QStringLiteral("started_ms"), jnum("started_ms"));
    receipt.insert(QStringLiteral("finished_ms"),
                   static_cast<double>(QDateTime::currentMSecsSinceEpoch()));
    receipt.insert(QStringLiteral("network"),
                   QJsonObject{{QStringLiteral("lez_sequencer"),
                                jstr(str(evidence, "lez_sequencer"))},
                               {QStringLiteral("eth_rpc"),
                                jstr(str(evidence, "eth_rpc"))},
                               {QStringLiteral("eth_chain_id"),
                                jnum("eth_chain_id")}});
    if (!error.isEmpty()) {
        receipt.insert(QStringLiteral("error"), error);
    }
    return receipt;
}

void SwapUiPlugin::publishReceiptsProp()
{
    setReceiptsJson(QString::fromUtf8(
        QJsonDocument(m_receipts).toJson(QJsonDocument::Compact)));
}

// Surface first (the in-memory list drives the History tab even if the disk
// append fails), then append one compact JSONL line with the trace logger's
// open-append-flush discipline. A write failure is logged and otherwise
// swallowed — receipts must never take the UI down.
void SwapUiPlugin::journalReceipt(const QJsonObject& receipt)
{
    m_receipts.prepend(receipt);
    while (m_receipts.size() > kMaxSurfacedReceipts) {
        m_receipts.removeLast();
    }
    publishReceiptsProp();

    const QString baseDir = writableModuleDir();
    if (baseDir.isEmpty()) {
        // Per-profile root not resolved yet: buffer rather than append to the
        // shared ui-host tree. flushPendingReceipts() writes these once the
        // root arrives. The line is already reflected in m_receipts above, so
        // the History view is correct in the meantime.
        m_pendingReceiptLines.append(
            QString::fromUtf8(QJsonDocument(receipt).toJson(QJsonDocument::Compact)));
        swapUiTrace(QStringLiteral(
            "journalReceipt: buffering — per-profile persistence root not resolved yet"));
        return;
    }
    const QString path = QDir(baseDir).filePath(QStringLiteral("receipts.jsonl"));
    const QDir dir = QFileInfo(path).dir();
    if (!dir.exists() && !dir.mkpath(QStringLiteral("."))) {
        swapUiTrace(QStringLiteral("journalReceipt: mkpath failed for %1")
                        .arg(dir.absolutePath()));
        return;
    }
    QFile f(path);
    if (!f.open(QIODevice::WriteOnly | QIODevice::Append)) {
        swapUiTrace(QStringLiteral("journalReceipt: open failed for %1: %2")
                        .arg(path, f.errorString()));
        return;
    }
    f.write(QJsonDocument(receipt).toJson(QJsonDocument::Compact) + '\n');
    f.flush();
    swapUiTrace(QStringLiteral("journalReceipt: appended status=%1 role=%2")
                    .arg(valueString(receipt, QStringLiteral("status")),
                         valueString(receipt, QStringLiteral("role"))));
}

// Whole-file read is fine at journal scale (one line per swap); the file is
// oldest-first, the surfaced list newest-first and capped. Corrupt lines are
// skipped individually so one bad write never hides the rest of the history.
void SwapUiPlugin::loadReceiptsFromDisk()
{
    m_receipts = QJsonArray{};
    QFile f(receiptsReadPath());
    if (f.open(QIODevice::ReadOnly | QIODevice::Text)) {
        while (!f.atEnd()) {
            const QByteArray line = f.readLine().trimmed();
            if (line.isEmpty()) {
                continue;
            }
            const auto doc = QJsonDocument::fromJson(line);
            if (!doc.isObject()) {
                continue;
            }
            m_receipts.prepend(doc.object());
            while (m_receipts.size() > kMaxSurfacedReceipts) {
                m_receipts.removeLast();
            }
        }
        swapUiTrace(QStringLiteral("loadReceiptsFromDisk: loaded %1 receipts")
                        .arg(m_receipts.size()));
    }
    publishReceiptsProp();
}

void SwapUiPlugin::refreshHistory()
{
    loadReceiptsFromDisk();
}

void SwapUiPlugin::clearHistory()
{
    // Clear ONLY the per-profile journal; never remove the legacy shared file
    // (issue #99). Truncating a per-profile file (rather than deleting) also
    // shadows any legacy receipts, so the cleared state sticks across a reload
    // even before this profile has recorded a swap of its own.
    m_pendingReceiptLines.clear();
    const QString baseDir = writableModuleDir();
    if (!baseDir.isEmpty()) {
        const QDir dir(baseDir);
        if (!dir.exists() && !dir.mkpath(QStringLiteral("."))) {
            swapUiTrace(QStringLiteral("clearHistory: mkpath failed for %1")
                            .arg(dir.absolutePath()));
            setErrorMessage(QStringLiteral("Could not clear the swap history file"));
            setStatus(errorMessage());
            return;
        }
        QFile f(dir.filePath(QStringLiteral("receipts.jsonl")));
        if (!f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            swapUiTrace(QStringLiteral("clearHistory: truncate failed for %1: %2")
                            .arg(f.fileName(), f.errorString()));
            setErrorMessage(QStringLiteral("Could not clear the swap history file"));
            setStatus(errorMessage());
            return;
        }
        f.close();
    } else {
        swapUiTrace(QStringLiteral(
            "clearHistory: per-profile root unknown; clearing in-memory history only"));
    }
    m_receipts = QJsonArray{};
    publishReceiptsProp();
    setStatus(QStringLiteral("Swap history cleared"));
}

void SwapUiPlugin::publishOffer()
{
    if (publishingLoading() || makerRunning() || takerRunning()) {
        return;
    }
    if (!validateConfigForAction(QStringLiteral("publish"))) {
        return;
    }
    ensureMessagingReady([this]() {
        setPublishingLoading(true);
        setStatus(QStringLiteral("Publishing offer..."));
        m_swap->publishOfferAsync(configJson(), [this](QString result) {
            setPublishingLoading(false);
            setOfferResultJson(result);
            setResultStatus(result,
                            QStringLiteral("Offer published"),
                            QStringLiteral("Offer publish failed"));
        });
    });
}

void SwapUiPlugin::fetchOffers()
{
    if (offersLoading() || makerRunning() || takerRunning() || autoAcceptRunning()) {
        return;
    }
    // Browsing only needs validateForBrowse (network constants), not the
    // full validateConfigForAction("offers") this used to call: a zero-config
    // fresh install must be able to see the market before typing anything
    // (see feat/browse-before-config). fetchOffersAsync itself takes no
    // config argument at all, so this was never more than an over-eager
    // gate.
    if (!validateForBrowse()) {
        setErrorMessage(QStringLiteral(
            "Network configuration incomplete — check RPC/sequencer URLs and HTLC IDs in Config"));
        setStatus(errorMessage());
        return;
    }
    ensureMessagingReady([this]() {
        setOffersLoading(true);
        setStatus(QStringLiteral("Fetching offers..."));
        m_swap->fetchOffersAsync([this](QString result) {
            setOffersLoading(false);
            setOffersJson(result);
            setResultStatus(result,
                            QStringLiteral("Offers fetched"),
                            QStringLiteral("Offer fetch failed"));
        });
    });
}

void SwapUiPlugin::startAutoAccept()
{
    if (!m_swap || autoAcceptRunning() || makerRunning() || takerRunning()) {
        return;
    }
    if (!validateConfigForAction(QStringLiteral("auto_accept"))) {
        return;
    }
    if (!subscribeToSwapEvents()) {
        setErrorMessage(QStringLiteral("Cannot start auto-accept: swap event subscription unavailable"));
        setStatus(errorMessage());
        return;
    }

    ensureMessagingReady([this]() {
        setPublishingLoading(true);
        setStatus(QStringLiteral("Publishing offer..."));
        m_swap->publishOfferAsync(configJson(), [this](QString result) {
            setPublishingLoading(false);
            setOfferResultJson(result);
            const auto error = jsonError(result);
            if (!error.isEmpty()) {
                setResultStatus(result,
                                QStringLiteral("Offer published"),
                                QStringLiteral("Offer publish failed"));
                return;
            }

            setAutoAcceptRunning(true);
            setMakerRunning(true);
            setAutoAcceptJobId(QString{});
            setMakerJobId(QString{});
            setAutoAcceptCompleted(0);
            setAutoAcceptFailed(0);
            setAutoAcceptIteration(0);
            setSwapHistory(QStringList{});
            beginMakerEvidence(false);
            clearMakerProgress();
            setMakerCurrentStep(QStringLiteral("WaitingForEthLock"));
            addMakerProgressStep(QStringLiteral("WaitingForEthLock"));
            setStatus(QStringLiteral("Offer published; live maker listener running"));
            setBusyState();

            m_swap->startMakerLoopJobAsync(configJson(), [this](QString startResult) {
                handleJobStartResult(QStringLiteral("maker_loop"), startResult);
            });
        });
    });
}

void SwapUiPlugin::stopAutoAccept()
{
    if (!m_swap || !autoAcceptRunning()) {
        return;
    }

    setStatus(QStringLiteral("Stopping live maker listener..."));
    if (!autoAcceptJobId().isEmpty()) {
        m_swap->stopJobAsync(autoAcceptJobId(), [this](QString result) {
            const auto error = jsonError(result);
            if (!error.isEmpty()) {
                setErrorMessage(error);
                setStatus(QStringLiteral("Stop failed: %1").arg(error));
            }
        });
    } else {
        m_swap->stopMakerLoop();
    }
}

QString SwapUiPlugin::normaliseHashlock(const QString& raw)
{
    auto trimmed = raw.trimmed();
    if (trimmed.startsWith(QStringLiteral("0x"), Qt::CaseInsensitive)) {
        trimmed = trimmed.mid(2);
    }
    if (trimmed.size() != 64) {
        return {};
    }
    static const QRegularExpression re(QStringLiteral("^[0-9a-fA-F]{64}$"));
    if (!re.match(trimmed).hasMatch()) {
        return {};
    }
    return trimmed.toLower();
}

void SwapUiPlugin::coordinationStart(const QString& role, const QString& hashlockHex)
{
    if (!m_swap) {
        return;
    }
    const auto canonical = normaliseHashlock(hashlockHex);
    if (canonical.isEmpty()) {
        return;
    }
    const auto previous = coordinationActiveHashlock();
    if (previous == canonical && m_coordinationRole == role) {
        return;
    }
    if (!previous.isEmpty()) {
        // Clean up the prior subscription before adopting a new hashlock.
        // Best-effort: ignore the response.
        m_swap->unsubscribeSwapAsync(previous, [](QString){});
    }

    m_coordinationRole = role;
    m_coordinationTakerPublished = false;
    setCoordinationActiveHashlock(canonical);
    setCoordinationEventsJson(QStringLiteral("[]"));
    setCoordinationLastResultJson(QString{});

    ensureMessagingReady([this, canonical]() {
        if (!m_swap || coordinationActiveHashlock() != canonical) {
            return;
        }
        m_swap->subscribeSwapAsync(canonical, [this, canonical](QString result) {
            if (coordinationActiveHashlock() != canonical) {
                return;
            }
            setCoordinationLastResultJson(result);
            const auto error = jsonError(result);
            if (!error.isEmpty()) {
                qWarning() << "swap-ui: subscribeSwap failed:" << error;
                return;
            }
            // Drain immediately in case Delivery already buffered events
            // between subscribe and our first poll tick.
            coordinationPollSwapEvents();
            if (!m_coordinationPollTimer.isActive()) {
                m_coordinationPollTimer.start();
            }
        });
    });
}

void SwapUiPlugin::coordinationStop()
{
    m_coordinationPollTimer.stop();
    const auto previous = coordinationActiveHashlock();
    m_coordinationRole.clear();
    m_coordinationTakerPublished = false;
    setCoordinationActiveHashlock(QString{});
    if (m_swap && !previous.isEmpty()) {
        m_swap->unsubscribeSwapAsync(previous, [](QString){});
    }
}

void SwapUiPlugin::coordinationPollSwapEvents()
{
    if (!m_swap) {
        return;
    }
    const auto active = coordinationActiveHashlock();
    if (active.isEmpty()) {
        m_coordinationPollTimer.stop();
        return;
    }
    m_swap->fetchSwapEventsAsync(active, [this, active](QString result) {
        if (coordinationActiveHashlock() != active) {
            return;
        }
        setCoordinationLastResultJson(result);
        const auto obj = parseObject(result);
        if (obj.value(QStringLiteral("ok")).toBool(false)) {
            const auto events = obj.value(QStringLiteral("events")).toArray();
            if (!events.isEmpty()) {
                coordinationAppendEvents(events);
            }
        }
    });
}

void SwapUiPlugin::coordinationAppendEvents(const QJsonArray& events)
{
    if (events.isEmpty()) {
        return;
    }
    auto current = QJsonDocument::fromJson(coordinationEventsJson().toUtf8()).array();
    for (const auto& event : events) {
        current.append(event);
        // The taker's Delivery SwapAccept is the only place the maker learns
        // the counterparty's identity — fold it into the receipt evidence.
        if (m_coordinationRole == QStringLiteral("maker") && event.isObject()) {
            const auto obj = event.toObject();
            const auto ethSwapId = valueString(obj, QStringLiteral("eth_swap_id"));
            if (!ethSwapId.isEmpty()) {
                const auto takerEth = valueString(obj, QStringLiteral("taker_eth_address"));
                const auto takerLez = valueString(obj, QStringLiteral("taker_lez_account"));
                if (!takerEth.isEmpty()) {
                    m_makerEvidence.insert(QStringLiteral("counterparty_eth"), takerEth);
                }
                if (!takerLez.isEmpty()) {
                    m_makerEvidence.insert(QStringLiteral("counterparty_lez"), takerLez);
                }
                if (valueString(m_makerEvidence, QStringLiteral("eth_swap_id")).isEmpty()) {
                    m_makerEvidence.insert(QStringLiteral("eth_swap_id"), ethSwapId);
                }
            }
        }
    }
    // Cap the surfaced list so a long-lived auto-accept loop does not bloat
    // the QML property.
    while (current.size() > 64) {
        current.removeFirst();
    }
    setCoordinationEventsJson(
        QString::fromUtf8(QJsonDocument(current).toJson(QJsonDocument::Compact)));
}

void SwapUiPlugin::coordinationPublishTakerAccept(const QString& hashlockHex,
                                                  const QString& ethSwapId)
{
    if (!m_swap) {
        return;
    }
    if (m_coordinationRole != QStringLiteral("taker")) {
        return;
    }
    if (m_coordinationTakerPublished) {
        return;
    }
    const auto canonical = normaliseHashlock(hashlockHex);
    if (canonical.isEmpty() || ethSwapId.trimmed().isEmpty()) {
        return;
    }
    const auto takerLezAccount = lezAccount().isEmpty() ? lezAccountId() : lezAccount();
    const auto takerEthAddress = ethAddress();
    if (takerLezAccount.trimmed().isEmpty() || takerEthAddress.trimmed().isEmpty()) {
        // Without resolved balances we don't know the taker's own
        // addresses; skip publishing rather than sending a malformed
        // SwapAccept payload.
        qWarning() << "swap-ui: skipping publishSwapAccept; balances not resolved yet";
        return;
    }

    QJsonObject accept{
        {QStringLiteral("hashlock"), canonical},
        {QStringLiteral("eth_swap_id"), ethSwapId},
        {QStringLiteral("taker_lez_account"), takerLezAccount},
        {QStringLiteral("taker_eth_address"), takerEthAddress}
    };

    m_coordinationTakerPublished = true;
    ensureMessagingReady([this, accept, canonical]() {
        if (!m_swap || coordinationActiveHashlock() != canonical) {
            m_coordinationTakerPublished = false;
            return;
        }
        m_swap->publishSwapAcceptAsync(compactJson(accept), [this](QString result) {
            setCoordinationLastResultJson(result);
            const auto error = jsonError(result);
            if (!error.isEmpty()) {
                m_coordinationTakerPublished = false;
                qWarning() << "swap-ui: publishSwapAccept failed:" << error;
            }
        });
    });
}
