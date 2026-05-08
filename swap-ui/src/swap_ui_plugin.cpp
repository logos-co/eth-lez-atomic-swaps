#include "swap_ui_plugin.h"

#include "logos_api.h"
#include "logos_api_client.h"
#include "swap_api.h"

#include <QDateTime>
#include <QDir>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonValue>
#include <QMetaObject>
#include <QMetaType>
#include <QRegularExpression>
#include <QVariant>
#include <QDebug>

namespace {

constexpr const char* kSwapModuleName = "swap";

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
    setWakuBootstrapMultiaddr(QString{});

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
    setOffersJson(QString{});
    setOfferResultJson(QString{});
    setBalancesLoading(false);
    setMessagingLoading(false);
    setOffersLoading(false);
    setPublishingLoading(false);
    setRefundsLoading(false);

    m_messagingPollTimer.setInterval(2000);
    connect(&m_messagingPollTimer, &QTimer::timeout,
            this, &SwapUiPlugin::pollMessagingStatus);
    validateConfig();
}

SwapUiPlugin::~SwapUiPlugin()
{
    m_messagingPollTimer.stop();
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
    subscribeToSwapEvents();
    m_messagingPollTimer.start();
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
    if (!wakuBootstrapMultiaddr().isEmpty()) {
        obj[QStringLiteral("waku_bootstrap_multiaddr")] = wakuBootstrapMultiaddr();
    }
    return compactJson(obj);
}

QString SwapUiPlugin::messagingConfigJson() const
{
    QJsonObject obj;
    obj[QStringLiteral("bootstrap_multiaddr")] = wakuBootstrapMultiaddr();
    obj[QStringLiteral("listen_port")] = 0;
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
    setIfPresent(QStringLiteral("waku_bootstrap_multiaddr"), &SwapUiPlugin::setWakuBootstrapMultiaddr);

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
    if (!wakuBootstrapMultiaddr().trimmed().isEmpty()
        && !wakuBootstrapMultiaddr().trimmed().startsWith(QStringLiteral("/"))) {
        addValidationError(errors, QStringLiteral("waku_bootstrap_multiaddr"), QStringLiteral("Must be a multiaddr"));
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

    if ((action == QStringLiteral("messaging") || action == QStringLiteral("publish")
         || action == QStringLiteral("offers") || action == QStringLiteral("auto_accept"))
        && wakuBootstrapMultiaddr().trimmed().isEmpty()) {
        addValidationError(errors, QStringLiteral("waku_bootstrap_multiaddr"),
                           QStringLiteral("Required for messaging"));
        ok = false;
    }
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
    if (eventName.startsWith(QStringLiteral("maker_loop."))) {
        activeJobId = autoAcceptJobId();
    } else if (eventName.startsWith(QStringLiteral("maker."))) {
        activeJobId = makerJobId();
    } else if (eventName.startsWith(QStringLiteral("taker."))) {
        activeJobId = takerJobId();
    }

    if (activeJobId.isEmpty()) {
        return false;
    }
    return !jobId.isEmpty() && jobId == activeJobId;
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
    else if (key == QStringLiteral("waku_bootstrap_multiaddr")) {
        setWakuBootstrapMultiaddr(value);
        setMessagingConnected(false);
        setMessagingPeerCount(0);
    } else {
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
    if (!wakuBootstrapMultiaddr().isEmpty()) {
        initMessaging();
    }
}

void SwapUiPlugin::loadEnvFile(const QString& path, const QString& role)
{
    if (!m_swap || balancesLoading() || messagingLoading() || offersLoading()
        || publishingLoading() || refundsLoading() || running()) {
        return;
    }

    const QString resolvedPath = resolveLocalEnvPath(path);

    setStatus(QStringLiteral("Loading %1...").arg(resolvedPath));
    setErrorMessage(QString{});
    m_swap->loadEnvAsync(resolvedPath, [this, role](QString result) {
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
        validateConfig();
        setStatus(QStringLiteral("Config loaded from env"));
        if (!wakuBootstrapMultiaddr().isEmpty()) {
            initMessaging();
        }
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
    setResultStatus(resultJson,
                    QStringLiteral("Maker swap finished"),
                    QStringLiteral("Maker swap failed"));
    fetchBalances();
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
    setResultStatus(resultJson,
                    QStringLiteral("Taker swap finished"),
                    QStringLiteral("Taker swap failed"));
    fetchBalances();
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
    setResultStatus(resultJson,
                    QStringLiteral("Auto-accept stopped"),
                    QStringLiteral("Auto-accept failed"));
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
    if (role == QStringLiteral("maker")) {
        setMakerJobId(jobId);
        setMakerRunning(true);
        setStatus(QStringLiteral("Maker swap running"));
    } else if (role == QStringLiteral("taker")) {
        setTakerJobId(jobId);
        setTakerRunning(true);
        setStatus(QStringLiteral("Taker swap running"));
    } else if (role == QStringLiteral("maker_loop")) {
        setAutoAcceptJobId(jobId);
        setMakerJobId(jobId);
        setAutoAcceptRunning(true);
        setMakerRunning(true);
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
        fetchBalances();
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
        fetchBalances();
    });
}

void SwapUiPlugin::ensureMessagingReady(std::function<void()> continuation)
{
    if (!m_swap) {
        setStatus(QStringLiteral("Swap client not ready"));
        return;
    }
    if (messagingConnected()) {
        if (continuation) {
            continuation();
        }
        return;
    }
    if (wakuBootstrapMultiaddr().isEmpty()) {
        validateConfigForAction(QStringLiteral("messaging"));
        setErrorMessage(QStringLiteral("Messaging bootstrap multiaddr is required"));
        setStatus(errorMessage());
        return;
    }
    if (m_messagingInitInFlight) {
        setStatus(QStringLiteral("Messaging is connecting..."));
        return;
    }

    m_messagingInitInFlight = true;
    setMessagingLoading(true);
    setStatus(QStringLiteral("Connecting to messaging..."));
    m_swap->messagingInitAsync(messagingConfigJson(), [this, continuation](QString result) {
        m_messagingInitInFlight = false;
        setMessagingLoading(false);
        const auto error = jsonError(result);
        if (!error.isEmpty()) {
            setErrorMessage(error);
            setStatus(QStringLiteral("Messaging connect failed: %1").arg(error));
            return;
        }
        setErrorMessage(QString{});
        setStatus(QStringLiteral("Messaging connected"));
        pollMessagingStatus();
        if (continuation) {
            continuation();
        }
    });
}

void SwapUiPlugin::initMessaging()
{
    ensureMessagingReady();
}

void SwapUiPlugin::pollMessagingStatus()
{
    if (!m_swap || (wakuBootstrapMultiaddr().isEmpty() && !messagingConnected())) {
        return;
    }
    m_swap->messagingStatusAsync([this](QString result) {
        if (isErrorResult(result)) {
            return;
        }
        const auto obj = parseObject(result);
        setMessagingConnected(obj.value(QStringLiteral("connected")).toBool(false));
        setMessagingPeerCount(obj.value(QStringLiteral("peer_count")).toInt(0));
    });
}

void SwapUiPlugin::subscribeToSwapEvents()
{
    if (!m_logosAPI) {
        return;
    }
    LogosAPIClient* client = m_logosAPI->getClient(kSwapModuleName);
    if (!client) {
        setErrorMessage(QStringLiteral("swap event client unavailable"));
        return;
    }

    m_eventObject = client->requestObject(QString::fromUtf8(kSwapModuleName));
    if (!m_eventObject) {
        setErrorMessage(QStringLiteral("swap event object unavailable"));
        return;
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
}

void SwapUiPlugin::onSwapEventArgs(const QString& eventName, const QVariantList& args)
{
    onSwapEvent(eventName, payloadFromArgs(args));
}

void SwapUiPlugin::onSwapEvent(const QString& eventName, const QString& payloadJson)
{
    const auto payload = parseObject(payloadJson);
    if (payload.isEmpty()) {
        return;
    }
    if (eventName.endsWith(QStringLiteral(".progress"))) {
        handleProgressEvent(eventName, payload);
    } else if (eventName.endsWith(QStringLiteral(".finished"))) {
        handleFinishedEvent(eventName, payload);
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
        return;
    }

    if (eventName == QStringLiteral("taker.progress")) {
        if (!shouldHandleJobEvent(eventName, payload)) {
            return;
        }
        setTakerCurrentStep(step);
        addTakerProgressStep(step);
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

    ensureMessagingReady([this]() {
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
        setStatus(QStringLiteral("Live maker listener running"));
        setBusyState();

        m_swap->startMakerLoopJobAsync(configJson(), [this](QString result) {
            handleJobStartResult(QStringLiteral("maker_loop"), result);
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
