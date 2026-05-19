#include "swap_ui_plugin.h"

#include "logos_api.h"
#include "logos_api_client.h"
#include "swap_api.h"

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
#include <QVariant>
#include <QDebug>

#include <cstdio>

namespace {

constexpr const char* kSwapModuleName = "swap";

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

} // namespace

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

    setEthRpcUrl(QString{});
    setEthPrivateKey(QString{});
    setEthHtlcAddress(QString{});
    setLezSequencerUrl(QStringLiteral("http://localhost:8080"));
    setLezSigningKey(QString{});
    setLezWalletHome(QString{});
    setLezAccountId(QString{});
    setLezHtlcProgramId(QString{});
    setLezAmount(QStringLiteral("1"));
    setEthAmount(QStringLiteral("1"));
    setLezTimelockMinutes(QStringLiteral("5"));
    setEthTimelockMinutes(QStringLiteral("10"));
    setEthRecipientAddress(QString{});
    setLezTakerAccountId(QString{});
    setPollIntervalMs(QStringLiteral("2000"));

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
    setMessagingConnectionStatus(QString{});
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

    m_messagingPollTimer.setInterval(2000);
    connect(&m_messagingPollTimer, &QTimer::timeout,
            this, &SwapUiPlugin::pollMessagingStatus);

    m_coordinationPollTimer.setInterval(1000);
    m_coordinationPollTimer.setSingleShot(false);
    connect(&m_coordinationPollTimer, &QTimer::timeout,
            this, &SwapUiPlugin::coordinationPollSwapEvents);

    validateConfig();
}

SwapUiPlugin::~SwapUiPlugin()
{
    m_messagingPollTimer.stop();
    m_coordinationPollTimer.stop();
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
    swapUiTrace(QStringLiteral("initLogos: starting"));
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
}

void SwapUiPlugin::applyOfferObject(const QJsonObject& offer)
{
    setEthRecipientAddress(valueString(offer, QStringLiteral("maker_eth_address")));
    setLezAmount(valueString(offer, QStringLiteral("lez_amount")));
    setEthAmount(weiToEthValue(valueString(offer, QStringLiteral("eth_amount"))));
    setEthHtlcAddress(valueString(offer, QStringLiteral("eth_htlc_address")));
    setLezHtlcProgramId(valueString(offer, QStringLiteral("lez_htlc_program_id")));
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
    require(QStringLiteral("lez_taker_account_id"), lezTakerAccountId());
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

    setValidationErrorsJson(compactJson(errors));
    if (!ok) {
        setErrorMessage(QStringLiteral("Fix validation errors before continuing"));
        setStatus(errorMessage());
    }
    return ok;
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
        setBalancesLoading(false);
        applyBalancesResult(result);
    });
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
        setBalancesLoading(false);
        applyBalancesResult(result);
    });
}

void SwapUiPlugin::handleMakerFinished(const QString& resultJson)
{
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
    fetchBalancesFromLoadedEnv();
}

void SwapUiPlugin::handleTakerFinished(const QString& resultJson)
{
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
    fetchBalancesFromLoadedEnv();
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
    fetchBalancesFromLoadedEnv();
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
    applyOfferObject(offer);
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
    setMakerCurrentStep(QStringLiteral("Refunding"));
    addMakerProgressStep(QStringLiteral("Refunding"));
    setStatus(QStringLiteral("Refunding LEZ..."));
    setBusyState();

    m_swap->refundLezAsync(configJson(), hashlockHex, [this](QString result) {
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
    setTakerCurrentStep(QStringLiteral("Refunding"));
    addTakerProgressStep(QStringLiteral("Refunding"));
    setStatus(QStringLiteral("Refunding ETH..."));
    setBusyState();

    m_swap->refundEthAsync(configJson(), swapIdHex, [this](QString result) {
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
        setMessagingConnectionStatus(obj.value(QStringLiteral("connection_status")).toString());
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
        QStringLiteral("maker.finished"),
        QStringLiteral("taker.finished"),
        QStringLiteral("maker_loop.finished")
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
        clearMakerProgress();
        setMakerCurrentStep(QStringLiteral("WaitingForEthLock"));
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
    }
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
    if (!validateConfigForAction(QStringLiteral("offers"))) {
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
