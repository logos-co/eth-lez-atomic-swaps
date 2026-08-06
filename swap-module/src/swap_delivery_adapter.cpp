#include "swap_delivery_adapter.h"

#include <string>

namespace {

std::string jsonEscape(const std::string& raw)
{
    std::string out;
    out.reserve(raw.size() + 8);
    for (char c : raw) {
        switch (c) {
        case '\\': out += "\\\\"; break;
        case '"': out += "\\\""; break;
        case '\n': out += "\\n"; break;
        case '\r': out += "\\r"; break;
        case '\t': out += "\\t"; break;
        default:
            if (static_cast<unsigned char>(c) < 0x20) {
                out += "?";
            } else {
                out += c;
            }
        }
    }
    return out;
}

std::string jsonError(const std::string& message)
{
    return "{\"ok\":false,\"error\":\"" + jsonEscape(message) + "\"}";
}

} // namespace

// Deliberately Qt-free (like swapDeliveryEthAmountToWei's fallback twin) so it
// compiles identically in the header-less test build and stays a pure,
// unit-testable function.
int swapDeliveryParsePeerCount(const std::string& raw)
{
    const auto first = raw.find_first_not_of(" \t\n\r");
    if (first == std::string::npos) {
        return -1;
    }
    const auto last = raw.find_last_not_of(" \t\n\r");
    std::string s = raw.substr(first, last - first + 1);

    // getNodeInfo returns "a UTF-16 string containing UTF-8 serializable JSON
    // data" — a bare count may arrive wrapped as a JSON string ("\"3\"").
    if (s.size() >= 2 && s.front() == '"' && s.back() == '"') {
        s = s.substr(1, s.size() - 2);
    }
    if (s.empty()) {
        return -1;
    }

    // JSON array of peer descriptors: count its top-level elements.
    if (s.front() == '[') {
        if (s.back() != ']') {
            return -1;
        }
        int depth = 0;
        bool inString = false;
        bool escaped = false;
        bool content = false;
        int commas = 0;
        for (char c : s) {
            if (inString) {
                if (escaped) {
                    escaped = false;
                } else if (c == '\\') {
                    escaped = true;
                } else if (c == '"') {
                    inString = false;
                }
                continue;
            }
            if (c == '"') {
                inString = true;
                content = true;
            } else if (c == '[' || c == '{') {
                if (++depth > 1) {
                    content = true;
                }
            } else if (c == ']' || c == '}') {
                if (--depth < 0) {
                    return -1;
                }
            } else if (depth == 1) {
                if (c == ',') {
                    ++commas;
                } else if (c != ' ' && c != '\t' && c != '\n' && c != '\r') {
                    content = true;
                }
            }
        }
        if (depth != 0 || inString) {
            return -1;
        }
        return content ? commas + 1 : 0;
    }

    // Bare (or string-wrapped) non-negative integer.
    std::size_t consumed = 0;
    try {
        const long value = std::stol(s, &consumed, 10);
        if (consumed != s.size() || value < 0) {
            return -1;
        }
        return static_cast<int>(value);
    } catch (...) {
        return -1;
    }
}

#if __has_include("logos_api.h") && __has_include("logos_sdk.h") && __has_include("logos_types.h")

#include "logos_api.h"
#include "logos_sdk.h"
#include "logos_types.h"

#include <QByteArray>
#include <QDateTime>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonValue>
#include <QRegularExpression>
#include <QString>
#include <QStringList>
#include <QVariant>
#include <QVariantList>

#include <memory>
#include <mutex>
#include <unordered_map>

namespace {

constexpr const char* kOffersTopic = "/atomic-swaps/1/offers/json";
constexpr qsizetype kMaxEncodedOfferPayloadChars = 96 * 1024;
constexpr qsizetype kMaxOfferPayloadBytes = 64 * 1024;
constexpr qsizetype kMaxCachedOffers = 256;
constexpr qsizetype kMaxCachedSwapEventsPerSwap = 32;
constexpr qsizetype kMaxTrackedSwaps = 64;

// Fleet peer probing. messagingStatus() is polled every 2s by swap_ui; the
// remote getNodeInfo round trip is throttled to this interval so status stays
// cheap while the peer count stays fresh enough to notice isolation.
constexpr qint64 kPeerProbeIntervalMs = 5000;
// After the probe is flagged unsupported (an out-of-date delivery_module
// without the node-info API), retry only occasionally — every poll would just
// burn a CALLBACK_TIMEOUT wait each time.
constexpr qint64 kPeerProbeUnsupportedRetryMs = 60 * 1000;
// Consecutive probe failures before concluding the installed delivery_module
// simply lacks the API (as opposed to a transient hiccup).
constexpr int kPeerProbeFailureLimit = 3;
// A confirmed zero-peer reading only raises the isolation hint after this
// long since start(): dialing the fleet bootstrap takes a while, and "0
// peers" in the first seconds of a healthy startup is normal, not isolation.
constexpr qint64 kZeroPeerAlarmGraceMs = 45 * 1000;

struct DeliveryState {
    std::mutex operationMutex;
    std::recursive_mutex mutex;
    LogosAPI* api = nullptr;
    std::shared_ptr<LogosModules> modules;
    bool nodeCreated = false;
    bool started = false;
    bool subscribed = false;
    QString connectionStatus;
    QString lastError;
    QJsonArray offers;
    // Per-swap coordination state. Keyed by canonical (lowercase, no 0x)
    // hashlock hex. Each entry is a FIFO of decoded SwapAccept-shaped
    // payloads delivered on /atomic-swaps/1/swap-<hashlock>/json.
    std::unordered_map<std::string, QJsonArray> swapEvents;
    std::unordered_map<std::string, bool> swapSubscriptions;
    // Fleet visibility (live incident: a stale delivery_module left the node
    // up + subscribed but attached to ZERO fleet peers, and the UI said
    // "Delivery connected" forever with an empty board). -1 = not yet known.
    int fleetPeerCount = -1;
    int peerProbeFailures = 0;
    // The installed delivery_module repeatedly failed getNodeInfo — treat it
    // as predating the node-info API (i.e. months out of date).
    bool peerInfoUnsupported = false;
    qint64 lastPeerProbeMs = 0;
    // When start() last succeeded; gates the zero-peer isolation hint (see
    // kZeroPeerAlarmGraceMs).
    qint64 startedAtMs = 0;
    // liblogosdelivery's self-reported version (getNodeInfo("Version")),
    // surfaced in messagingStatus as a diagnostic.
    QString deliveryVersion;
};

DeliveryState& state()
{
    static DeliveryState s;
    return s;
}

std::string compactJson(const QJsonObject& obj)
{
    return QJsonDocument(obj).toJson(QJsonDocument::Compact).toStdString();
}

QJsonObject parseObject(const std::string& json)
{
    const auto doc = QJsonDocument::fromJson(QByteArray::fromStdString(json));
    return doc.isObject() ? doc.object() : QJsonObject{};
}

QStringList offerKeys()
{
    return {
        QStringLiteral("hashlock"),
        QStringLiteral("lez_amount"),
        QStringLiteral("eth_amount"),
        QStringLiteral("maker_eth_address"),
        QStringLiteral("maker_lez_account"),
        QStringLiteral("lez_timelock"),
        QStringLiteral("eth_timelock"),
        QStringLiteral("lez_htlc_program_id"),
        QStringLiteral("eth_htlc_address")
    };
}

std::string ethAmountToWeiValue(const std::string& ethAmount)
{
    QString value = QString::fromStdString(ethAmount).trimmed();
    if (value.isEmpty()) {
        return "0";
    }

    const int dot = value.indexOf(QLatin1Char('.'));
    QString whole = dot >= 0 ? value.left(dot) : value;
    QString fraction = dot >= 0 ? value.mid(dot + 1) : QString{};

    if (fraction.length() > 18) {
        fraction.truncate(18);
    }
    while (fraction.length() < 18) {
        fraction.append(QLatin1Char('0'));
    }

    QString wei = whole + fraction;
    int firstNonZero = 0;
    while (firstNonZero + 1 < wei.size() && wei.at(firstNonZero) == QLatin1Char('0')) {
        ++firstNonZero;
    }
    return wei.mid(firstNonZero).toStdString();
}

void normalizeOfferEthAmount(QJsonObject& offer)
{
    if (!offer.contains(QStringLiteral("eth_amount"))) {
        return;
    }
    const auto ethAmount = offer.value(QStringLiteral("eth_amount"))
                               .toVariant()
                               .toString()
                               .toStdString();
    offer.insert(QStringLiteral("eth_amount"),
                 QString::fromStdString(ethAmountToWeiValue(ethAmount)));
}

void copyIfPresent(QJsonObject& out,
                   const QString& outKey,
                   const QJsonObject& input,
                   const QString& inputKey)
{
    if (!out.contains(outKey) && input.contains(inputKey)) {
        out.insert(outKey, input.value(inputKey));
    }
}

void copyTimelockMinutes(QJsonObject& out,
                         const QString& outKey,
                         const QJsonObject& input,
                         const QString& minutesKey)
{
    if (out.contains(outKey) || !input.contains(minutesKey)) {
        return;
    }
    bool ok = false;
    const qint64 minutes = input.value(minutesKey).toVariant().toLongLong(&ok);
    if (ok && minutes > 0) {
        out.insert(outKey, QDateTime::currentSecsSinceEpoch() + minutes * 60);
    }
}

QJsonObject filteredOfferFields(const QJsonObject& source)
{
    QJsonObject offer;
    for (const QString& key : offerKeys()) {
        if (source.contains(key)) {
            offer.insert(key, source.value(key));
        }
    }
    return offer;
}

bool hasOfferCoreFields(const QJsonObject& offer)
{
    const QStringList required{
        QStringLiteral("lez_amount"),
        QStringLiteral("eth_amount"),
        QStringLiteral("maker_eth_address"),
        QStringLiteral("maker_lez_account"),
        QStringLiteral("lez_timelock"),
        QStringLiteral("eth_timelock"),
        QStringLiteral("lez_htlc_program_id"),
        QStringLiteral("eth_htlc_address")
    };
    for (const QString& key : required) {
        if (!offer.contains(key)) {
            return false;
        }
    }
    return true;
}

// Canonical hashlock hex: lowercase, no 0x prefix, exactly 64 hex chars
// (32 bytes). Returns empty string if the input is malformed.
std::string canonicalHashlockHex(const std::string& raw)
{
    std::string s = raw;
    if (s.size() >= 2 && s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) {
        s.erase(0, 2);
    }
    if (s.size() != 64) {
        return {};
    }
    for (char& c : s) {
        if (c >= 'A' && c <= 'F') {
            c = static_cast<char>(c - 'A' + 'a');
        } else if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) {
            return {};
        }
    }
    return s;
}

QString swapTopicForHashlock(const std::string& canonicalHashlock)
{
    return QStringLiteral("/atomic-swaps/1/swap-%1/json")
        .arg(QString::fromStdString(canonicalHashlock));
}

std::string canonicalHashlockFromSwapTopic(const QString& topic)
{
    static const QRegularExpression re(QStringLiteral(
        "^/atomic-swaps/1/swap-([0-9a-fA-F]{64})/json$"));
    const auto match = re.match(topic);
    if (!match.hasMatch()) {
        return {};
    }
    return canonicalHashlockHex(match.captured(1).toStdString());
}

QString deliveryConfigJson(const std::string& configJson)
{
    const QJsonObject input = parseObject(configJson);
    const QJsonValue delivery = input.value(QStringLiteral("delivery"));
    if (delivery.isObject()) {
        return QString::fromUtf8(QJsonDocument(delivery.toObject()).toJson(QJsonDocument::Compact));
    }

    QJsonObject cfg{
        {QStringLiteral("logLevel"), input.value(QStringLiteral("logLevel")).toString(QStringLiteral("INFO"))},
        {QStringLiteral("mode"), input.value(QStringLiteral("mode")).toString(QStringLiteral("Core"))},
        {QStringLiteral("preset"), input.value(QStringLiteral("preset")).toString(QStringLiteral("logos.dev"))}
    };
    if (input.contains(QStringLiteral("portsShift"))) {
        cfg.insert(QStringLiteral("portsShift"), input.value(QStringLiteral("portsShift")));
    }
    return QString::fromUtf8(QJsonDocument(cfg).toJson(QJsonDocument::Compact));
}

QStringList swapAcceptKeys()
{
    return {
        QStringLiteral("hashlock"),
        QStringLiteral("eth_swap_id"),
        QStringLiteral("taker_lez_account"),
        QStringLiteral("taker_eth_address")
    };
}

QJsonObject filteredSwapAcceptFields(const QJsonObject& source)
{
    QJsonObject out;
    for (const QString& key : swapAcceptKeys()) {
        if (source.contains(key)) {
            out.insert(key, source.value(key));
        }
    }
    return out;
}

bool hasSwapAcceptCoreFields(const QJsonObject& accept)
{
    for (const QString& key : swapAcceptKeys()) {
        if (!accept.contains(key) || !accept.value(key).isString()
            || accept.value(key).toString().trimmed().isEmpty()) {
            return false;
        }
    }
    return true;
}

QJsonObject offerPayload(const std::string& configJson)
{
    const QJsonObject input = parseObject(configJson);
    if (input.contains(QStringLiteral("offer")) && input.value(QStringLiteral("offer")).isObject()) {
        QJsonObject offer = filteredOfferFields(input.value(QStringLiteral("offer")).toObject());
        if (!offer.contains(QStringLiteral("hashlock"))) {
            offer.insert(QStringLiteral("hashlock"), QString{});
        }
        return offer;
    }

    QJsonObject offer = filteredOfferFields(input);
    normalizeOfferEthAmount(offer);
    copyIfPresent(offer, QStringLiteral("maker_eth_address"), input, QStringLiteral("eth_recipient_address"));
    copyIfPresent(offer, QStringLiteral("maker_lez_account"), input, QStringLiteral("lez_account_id"));
    copyTimelockMinutes(offer, QStringLiteral("lez_timelock"), input, QStringLiteral("lez_timelock_minutes"));
    copyTimelockMinutes(offer, QStringLiteral("eth_timelock"), input, QStringLiteral("eth_timelock_minutes"));
    if (!offer.contains(QStringLiteral("hashlock"))) {
        offer.insert(QStringLiteral("hashlock"), QString{});
    }
    return offer;
}

void recordDeliveryError(const QString& error)
{
    DeliveryState& s = state();
    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    s.lastError = error;
}

void wireEventsLocked(DeliveryState& s)
{
    s.modules->delivery_module.on("connectionStateChanged", [](const QVariantList& data) {
        if (data.isEmpty()) return;
        DeliveryState& st = state();
        std::lock_guard<std::recursive_mutex> lock(st.mutex);
        st.connectionStatus = data.at(0).toString();
    });

    s.modules->delivery_module.on("messageReceived", [](const QVariantList& data) {
        if (data.size() < 4) return;

        const QString contentTopic = data.at(1).toString();
        const QString encodedPayload = data.at(2).toString();
        if (encodedPayload.size() > kMaxEncodedOfferPayloadChars) return;
        const QByteArray decoded = QByteArray::fromBase64(encodedPayload.toUtf8());
        if (decoded.size() > kMaxOfferPayloadBytes) return;
        const auto doc = QJsonDocument::fromJson(decoded);
        if (!doc.isObject()) return;

        if (contentTopic == QString::fromUtf8(kOffersTopic)) {
            QJsonObject offer = filteredOfferFields(doc.object());
            if (!hasOfferCoreFields(offer)) return;
            offer.insert(QStringLiteral("message_hash"), data.at(0).toString());
            offer.insert(QStringLiteral("timestamp_ms"), QDateTime::currentMSecsSinceEpoch());

            DeliveryState& st = state();
            std::lock_guard<std::recursive_mutex> lock(st.mutex);
            while (st.offers.size() >= kMaxCachedOffers) {
                st.offers.removeAt(0);
            }
            st.offers.append(offer);
            return;
        }

        const std::string topicHashlock = canonicalHashlockFromSwapTopic(contentTopic);
        if (topicHashlock.empty()) return;

        QJsonObject accept = filteredSwapAcceptFields(doc.object());
        if (!hasSwapAcceptCoreFields(accept)) return;
        // Drop messages whose embedded hashlock disagrees with the topic
        // they arrived on — never trust a self-described hashlock alone.
        const std::string payloadHashlock = canonicalHashlockHex(
            accept.value(QStringLiteral("hashlock")).toString().toStdString());
        if (payloadHashlock != topicHashlock) return;
        accept.insert(QStringLiteral("hashlock"),
                      QString::fromStdString(topicHashlock));
        accept.insert(QStringLiteral("message_hash"), data.at(0).toString());
        accept.insert(QStringLiteral("timestamp_ms"), QDateTime::currentMSecsSinceEpoch());

        DeliveryState& st = state();
        std::lock_guard<std::recursive_mutex> lock(st.mutex);
        // Only retain events for swaps the maker explicitly subscribed to.
        // This avoids unbounded memory if Delivery delivers messages for
        // topics we have already unsubscribed from.
        if (st.swapSubscriptions.find(topicHashlock) == st.swapSubscriptions.end()) {
            return;
        }
        QJsonArray& bucket = st.swapEvents[topicHashlock];
        while (bucket.size() >= kMaxCachedSwapEventsPerSwap) {
            bucket.removeAt(0);
        }
        bucket.append(accept);
    });

    s.modules->delivery_module.on("messageError", [](const QVariantList& data) {
        if (data.size() < 3) return;
        recordDeliveryError(data.at(2).toString());
    });
}

std::string logosError(const QString& op, const LogosResult& result)
{
    return jsonError(QStringLiteral("%1 failed: %2").arg(op, result.getError()).toStdString());
}

// Refresh the cached fleet peer count (and, once, the liblogosdelivery
// version) via the delivery module's node-info API. Called from
// swapDeliveryMessagingStatus(); throttled by kPeerProbeIntervalMs.
//
// Why this API: the module dependency schema has no version constraints
// (liblogos DependencyResolver resolves plain names only), so an out-of-date
// delivery_module cannot be rejected at load time. At runtime the module
// exposes getNodeInfo IDs including relayPeerCount / peerCount (verified
// against liblogosdelivery and delivery-module v0.1.1's Q_INVOKABLE surface;
// the plugin's own version() is NOT remotely invokable). So:
//  - a healthy module answers with a live count → zero-peer fleet isolation
//    becomes visible instead of hiding behind "Delivery connected";
//  - a months-old module fails the call entirely → flagged as out of date.
void probeFleetPeers()
{
    DeliveryState& s = state();
    std::shared_ptr<LogosModules> modules;
    bool needVersion = false;
    {
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        if (!s.modules || !s.started) {
            return;
        }
        const qint64 now = QDateTime::currentMSecsSinceEpoch();
        const qint64 interval =
            s.peerInfoUnsupported ? kPeerProbeUnsupportedRetryMs : kPeerProbeIntervalMs;
        if (now - s.lastPeerProbeMs < interval) {
            return;
        }
        s.lastPeerProbeMs = now;
        modules = s.modules;
        needVersion = s.deliveryVersion.isEmpty();
    }

    // Status polling must stay cheap: never queue behind a long-running
    // delivery operation (createNode can take seconds). If an operation is
    // in flight, skip this probe and keep the cached value.
    std::unique_lock<std::mutex> opLock(s.operationMutex, std::try_to_lock);
    if (!opLock.owns_lock()) {
        return;
    }

    // relayPeerCount = gossipsub mesh peers, i.e. what actually carries
    // offers in Core mode; fall back to the total peerCount for
    // liblogosdelivery builds that don't expose the relay item.
    int count = -1;
    const LogosResult relay =
        modules->delivery_module.getNodeInfo(QStringLiteral("relayPeerCount"));
    if (relay.success) {
        count = swapDeliveryParsePeerCount(relay.getString().toStdString());
    }
    if (count < 0) {
        const LogosResult total =
            modules->delivery_module.getNodeInfo(QStringLiteral("peerCount"));
        if (total.success) {
            count = swapDeliveryParsePeerCount(total.getString().toStdString());
        }
    }

    QString version;
    if (count >= 0 && needVersion) {
        const LogosResult v = modules->delivery_module.getNodeInfo(QStringLiteral("Version"));
        if (v.success) {
            version = v.getString().trimmed();
        }
    }

    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    if (count >= 0) {
        s.fleetPeerCount = count;
        s.peerProbeFailures = 0;
        s.peerInfoUnsupported = false;
        if (!version.isEmpty() && s.deliveryVersion.isEmpty()) {
            s.deliveryVersion = version;
        }
    } else {
        s.fleetPeerCount = -1;
        if (++s.peerProbeFailures >= kPeerProbeFailureLimit) {
            s.peerInfoUnsupported = true;
        }
    }
}

} // namespace

std::string swapDeliveryEthAmountToWei(const std::string& ethAmount)
{
    return ethAmountToWeiValue(ethAmount);
}

void swapDeliverySetRuntimeLogosAPI(void* api)
{
    DeliveryState& s = state();
    std::lock_guard<std::mutex> opLock(s.operationMutex);
    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    s.api = static_cast<LogosAPI*>(api);
    s.modules = s.api ? std::make_shared<LogosModules>(s.api) : nullptr;
    // The runtime stamps modulePath/instanceId/instancePersistencePath onto the
    // LogosAPI before any method dispatch, so the property is readable from here
    // on (see swapDeliveryRuntimePersistencePath()).
    s.nodeCreated = false;
    s.started = false;
    s.subscribed = false;
    s.connectionStatus.clear();
    s.lastError.clear();
    s.offers = QJsonArray{};
    s.swapEvents.clear();
    s.swapSubscriptions.clear();
    s.fleetPeerCount = -1;
    s.peerProbeFailures = 0;
    s.peerInfoUnsupported = false;
    s.lastPeerProbeMs = 0;
    s.startedAtMs = 0;
    s.deliveryVersion.clear();
    if (s.modules) {
        wireEventsLocked(s);
    }
}

std::string swapDeliveryRuntimePersistencePath()
{
    DeliveryState& s = state();
    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    if (!s.api) {
        return {};
    }
    return s.api->property("instancePersistencePath").toString().toStdString();
}

std::string swapDeliveryMessagingInit(const std::string& configJson)
{
    DeliveryState& s = state();
    std::lock_guard<std::mutex> opLock(s.operationMutex);
    std::shared_ptr<LogosModules> modules;
    bool needsCreate = false;
    bool needsStart = false;
    bool needsSubscribe = false;
    {
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        if (!s.modules) {
            return jsonError("delivery_module runtime is not initialized");
        }
        modules = s.modules;
        needsCreate = !s.nodeCreated;
        needsStart = !s.started;
        needsSubscribe = !s.subscribed;
    }

    if (needsCreate) {
        LogosResult created = modules->delivery_module.createNode(deliveryConfigJson(configJson));
        if (!created.success) {
            return logosError(QStringLiteral("createNode"), created);
        }
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        s.nodeCreated = true;
    }

    if (needsStart) {
        LogosResult started = modules->delivery_module.start();
        if (!started.success) {
            return logosError(QStringLiteral("start"), started);
        }
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        s.started = true;
        s.startedAtMs = QDateTime::currentMSecsSinceEpoch();
    }

    if (needsSubscribe) {
        LogosResult subscribed = modules->delivery_module.subscribe(QString::fromUtf8(kOffersTopic));
        if (!subscribed.success) {
            return logosError(QStringLiteral("subscribe"), subscribed);
        }
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        s.subscribed = true;
    }

    return R"({"ok":true,"method":"messagingInit","backend":"delivery_module"})";
}

std::string swapDeliveryMessagingShutdown()
{
    DeliveryState& s = state();
    std::lock_guard<std::mutex> opLock(s.operationMutex);
    std::shared_ptr<LogosModules> modules;
    bool needsUnsubscribe = false;
    bool needsStop = false;
    {
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        if (!s.modules) {
            return jsonError("delivery_module runtime is not initialized");
        }
        modules = s.modules;
        needsUnsubscribe = s.subscribed;
        needsStop = s.started;
    }

    if (needsUnsubscribe) {
        LogosResult unsubscribed = modules->delivery_module.unsubscribe(QString::fromUtf8(kOffersTopic));
        if (!unsubscribed.success) {
            return logosError(QStringLiteral("unsubscribe"), unsubscribed);
        }
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        s.subscribed = false;
    }

    if (needsStop) {
        LogosResult stopped = modules->delivery_module.stop();
        if (!stopped.success) {
            return logosError(QStringLiteral("stop"), stopped);
        }
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        s.started = false;
        // delivery_module.stop() drops every active subscription, so the
        // per-swap subscription map and any cached events are no longer
        // meaningful. Clear them to avoid leaking stale state across
        // restarts of the messaging stack. The peer count belongs to the
        // stopped node, so it goes back to unknown too.
        s.swapSubscriptions.clear();
        s.swapEvents.clear();
        s.fleetPeerCount = -1;
        s.peerProbeFailures = 0;
        s.lastPeerProbeMs = 0;
        s.startedAtMs = 0;
    }

    return R"({"ok":true,"method":"messagingShutdown","backend":"delivery_module"})";
}

std::string swapDeliveryMessagingStatus()
{
    // Refresh the fleet peer count first (throttled + try-lock internally, so
    // this stays cheap and never blocks behind a long delivery operation).
    probeFleetPeers();

    DeliveryState& s = state();
    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    QJsonObject status{
        {QStringLiteral("ok"), true},
        {QStringLiteral("method"), QStringLiteral("messagingStatus")},
        {QStringLiteral("backend"), QStringLiteral("delivery_module")},
        {QStringLiteral("connected"), s.started},
        {QStringLiteral("peer_count"), s.fleetPeerCount > 0 ? s.fleetPeerCount : 0},
        // false = the count above is a placeholder (probe not yet run, or the
        // installed delivery_module cannot answer it) — the UI must not read
        // it as a confirmed "zero peers".
        {QStringLiteral("peer_count_known"), s.fleetPeerCount >= 0},
        {QStringLiteral("connection_status"), s.connectionStatus},
        {QStringLiteral("swap_subscription_count"),
            static_cast<int>(s.swapSubscriptions.size())}
    };
    if (!s.deliveryVersion.isEmpty()) {
        status.insert(QStringLiteral("delivery_version"), s.deliveryVersion);
    }
    // Actionable, persistent hints for the two silent failure modes a merely
    // boolean "connected" masks: fleet isolation and a stale delivery_module.
    QString hint;
    if (s.started && s.subscribed) {
        if (s.peerInfoUnsupported) {
            hint = QStringLiteral(
                "delivery_module did not answer its peer-count API — it is "
                "likely out of date; update it in the Basecamp module manager");
        } else if (s.fleetPeerCount == 0
                   && QDateTime::currentMSecsSinceEpoch() - s.startedAtMs
                          > kZeroPeerAlarmGraceMs) {
            // Grace-gated: 0 peers seconds after start is normal dialing,
            // not isolation.
            hint = QStringLiteral(
                "connected to 0 fleet peers — offers cannot arrive; check "
                "that delivery_module is up to date");
        }
    }
    if (!hint.isEmpty()) {
        status.insert(QStringLiteral("delivery_hint"), hint);
    }
    if (!s.lastError.isEmpty()) {
        status.insert(QStringLiteral("last_error"), s.lastError);
    }
    return compactJson(status);
}

std::string swapDeliveryPublishOffer(const std::string& configJson)
{
    DeliveryState& s = state();
    std::lock_guard<std::mutex> opLock(s.operationMutex);
    std::shared_ptr<LogosModules> modules;
    {
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        if (!s.modules || !s.started || !s.subscribed) {
            return jsonError("messaging not initialized - call messagingInit first");
        }
        modules = s.modules;
    }

    const QJsonObject offer = offerPayload(configJson);
    if (!hasOfferCoreFields(offer)) {
        return jsonError("offer payload is missing required public fields");
    }
    const QByteArray payloadBytes = QJsonDocument(offer).toJson(QJsonDocument::Compact);
    if (payloadBytes.size() > kMaxOfferPayloadBytes) {
        return jsonError("offer payload is too large");
    }
    const QString payload = QString::fromUtf8(payloadBytes);
    LogosResult sent = modules->delivery_module.send(QString::fromUtf8(kOffersTopic), payload);
    if (!sent.success) {
        return logosError(QStringLiteral("send"), sent);
    }

    QJsonObject result{
        {QStringLiteral("ok"), true},
        {QStringLiteral("method"), QStringLiteral("publishOffer")},
        {QStringLiteral("backend"), QStringLiteral("delivery_module")},
        {QStringLiteral("request_id"), sent.getString()}
    };
    return compactJson(result);
}

std::string swapDeliveryFetchOffers()
{
    DeliveryState& s = state();
    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    QJsonObject result{
        {QStringLiteral("ok"), true},
        {QStringLiteral("method"), QStringLiteral("fetchOffers")},
        {QStringLiteral("backend"), QStringLiteral("delivery_module")},
        {QStringLiteral("offers"), s.offers}
    };
    s.offers = QJsonArray{};
    return compactJson(result);
}

std::string swapDeliverySubscribeSwap(const std::string& hashlockHex)
{
    const std::string canonical = canonicalHashlockHex(hashlockHex);
    if (canonical.empty()) {
        return jsonError("hashlock must be 32 bytes of hex");
    }

    DeliveryState& s = state();
    std::lock_guard<std::mutex> opLock(s.operationMutex);
    std::shared_ptr<LogosModules> modules;
    bool needsSubscribe = false;
    {
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        if (!s.modules || !s.started) {
            return jsonError("messaging not initialized - call messagingInit first");
        }
        if (s.swapSubscriptions.size() >= static_cast<std::size_t>(kMaxTrackedSwaps)
            && s.swapSubscriptions.find(canonical) == s.swapSubscriptions.end()) {
            return jsonError("too many active swap subscriptions");
        }
        modules = s.modules;
        needsSubscribe = !s.swapSubscriptions[canonical];
    }

    if (needsSubscribe) {
        LogosResult subscribed =
            modules->delivery_module.subscribe(swapTopicForHashlock(canonical));
        if (!subscribed.success) {
            std::lock_guard<std::recursive_mutex> lock(s.mutex);
            s.swapSubscriptions.erase(canonical);
            return logosError(QStringLiteral("subscribe"), subscribed);
        }
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        s.swapSubscriptions[canonical] = true;
    }

    QJsonObject result{
        {QStringLiteral("ok"), true},
        {QStringLiteral("method"), QStringLiteral("subscribeSwap")},
        {QStringLiteral("backend"), QStringLiteral("delivery_module")},
        {QStringLiteral("hashlock"), QString::fromStdString(canonical)},
        {QStringLiteral("topic"), swapTopicForHashlock(canonical)}
    };
    return compactJson(result);
}

std::string swapDeliveryUnsubscribeSwap(const std::string& hashlockHex)
{
    const std::string canonical = canonicalHashlockHex(hashlockHex);
    if (canonical.empty()) {
        return jsonError("hashlock must be 32 bytes of hex");
    }

    DeliveryState& s = state();
    std::lock_guard<std::mutex> opLock(s.operationMutex);
    std::shared_ptr<LogosModules> modules;
    bool needsUnsubscribe = false;
    {
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        if (!s.modules || !s.started) {
            return jsonError("messaging not initialized - call messagingInit first");
        }
        modules = s.modules;
        needsUnsubscribe = s.swapSubscriptions.count(canonical) > 0;
    }

    if (needsUnsubscribe) {
        LogosResult unsubscribed =
            modules->delivery_module.unsubscribe(swapTopicForHashlock(canonical));
        if (!unsubscribed.success) {
            return logosError(QStringLiteral("unsubscribe"), unsubscribed);
        }
    }

    {
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        s.swapSubscriptions.erase(canonical);
        s.swapEvents.erase(canonical);
    }

    QJsonObject result{
        {QStringLiteral("ok"), true},
        {QStringLiteral("method"), QStringLiteral("unsubscribeSwap")},
        {QStringLiteral("backend"), QStringLiteral("delivery_module")},
        {QStringLiteral("hashlock"), QString::fromStdString(canonical)}
    };
    return compactJson(result);
}

std::string swapDeliveryPublishSwapAccept(const std::string& configJson)
{
    DeliveryState& s = state();
    std::lock_guard<std::mutex> opLock(s.operationMutex);
    std::shared_ptr<LogosModules> modules;
    {
        std::lock_guard<std::recursive_mutex> lock(s.mutex);
        if (!s.modules || !s.started) {
            return jsonError("messaging not initialized - call messagingInit first");
        }
        modules = s.modules;
    }

    QJsonObject input = parseObject(configJson);
    if (input.contains(QStringLiteral("accept"))
        && input.value(QStringLiteral("accept")).isObject()) {
        input = input.value(QStringLiteral("accept")).toObject();
    }

    const std::string canonical = canonicalHashlockHex(
        input.value(QStringLiteral("hashlock")).toString().toStdString());
    if (canonical.empty()) {
        return jsonError("hashlock must be 32 bytes of hex");
    }

    QJsonObject accept = filteredSwapAcceptFields(input);
    accept.insert(QStringLiteral("hashlock"), QString::fromStdString(canonical));
    if (!hasSwapAcceptCoreFields(accept)) {
        return jsonError("swap accept payload is missing required fields");
    }

    const QByteArray payloadBytes = QJsonDocument(accept).toJson(QJsonDocument::Compact);
    if (payloadBytes.size() > kMaxOfferPayloadBytes) {
        return jsonError("swap accept payload is too large");
    }
    const QString payload = QString::fromUtf8(payloadBytes);
    LogosResult sent = modules->delivery_module.send(
        swapTopicForHashlock(canonical), payload);
    if (!sent.success) {
        return logosError(QStringLiteral("send"), sent);
    }

    QJsonObject result{
        {QStringLiteral("ok"), true},
        {QStringLiteral("method"), QStringLiteral("publishSwapAccept")},
        {QStringLiteral("backend"), QStringLiteral("delivery_module")},
        {QStringLiteral("hashlock"), QString::fromStdString(canonical)},
        {QStringLiteral("topic"), swapTopicForHashlock(canonical)},
        {QStringLiteral("request_id"), sent.getString()}
    };
    return compactJson(result);
}

std::string swapDeliveryFetchSwapEvents(const std::string& hashlockHex)
{
    const std::string canonical = canonicalHashlockHex(hashlockHex);
    if (canonical.empty()) {
        return jsonError("hashlock must be 32 bytes of hex");
    }

    DeliveryState& s = state();
    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    QJsonArray events;
    auto it = s.swapEvents.find(canonical);
    if (it != s.swapEvents.end()) {
        events = it->second;
        it->second = QJsonArray{};
    }

    QJsonObject result{
        {QStringLiteral("ok"), true},
        {QStringLiteral("method"), QStringLiteral("fetchSwapEvents")},
        {QStringLiteral("backend"), QStringLiteral("delivery_module")},
        {QStringLiteral("hashlock"), QString::fromStdString(canonical)},
        {QStringLiteral("subscribed"),
            s.swapSubscriptions.count(canonical) > 0},
        {QStringLiteral("events"), events}
    };
    return compactJson(result);
}

#else

void swapDeliverySetRuntimeLogosAPI(void*) {}

std::string swapDeliveryRuntimePersistencePath() { return {}; }

std::string swapDeliveryEthAmountToWei(const std::string& ethAmount)
{
    std::string value = ethAmount;
    const auto first = value.find_first_not_of(" \t\n\r");
    if (first == std::string::npos) {
        return "0";
    }
    const auto last = value.find_last_not_of(" \t\n\r");
    value = value.substr(first, last - first + 1);

    const auto dot = value.find('.');
    std::string whole = dot == std::string::npos ? value : value.substr(0, dot);
    std::string fraction = dot == std::string::npos ? std::string{} : value.substr(dot + 1);
    if (fraction.size() > 18) {
        fraction.resize(18);
    }
    while (fraction.size() < 18) {
        fraction.push_back('0');
    }

    std::string wei = whole + fraction;
    const auto nonZero = wei.find_first_not_of('0');
    return nonZero == std::string::npos ? std::string("0") : wei.substr(nonZero);
}

std::string swapDeliveryMessagingInit(const std::string&)
{
    return jsonError("delivery_module runtime is not available in this build");
}

std::string swapDeliveryMessagingShutdown()
{
    return jsonError("delivery_module runtime is not available in this build");
}

std::string swapDeliveryMessagingStatus()
{
    return R"({"ok":true,"method":"messagingStatus","backend":"delivery_module","connected":false,"peer_count":0,"peer_count_known":false,"unavailable":true})";
}

std::string swapDeliveryPublishOffer(const std::string&)
{
    return jsonError("messaging not initialized - call messagingInit first");
}

std::string swapDeliveryFetchOffers()
{
    return R"({"ok":true,"method":"fetchOffers","backend":"delivery_module","offers":[],"unavailable":true})";
}

std::string swapDeliverySubscribeSwap(const std::string&)
{
    return jsonError("messaging not initialized - call messagingInit first");
}

std::string swapDeliveryUnsubscribeSwap(const std::string&)
{
    return jsonError("messaging not initialized - call messagingInit first");
}

std::string swapDeliveryPublishSwapAccept(const std::string&)
{
    return jsonError("messaging not initialized - call messagingInit first");
}

std::string swapDeliveryFetchSwapEvents(const std::string&)
{
    return R"({"ok":true,"method":"fetchSwapEvents","backend":"delivery_module","events":[],"subscribed":false,"unavailable":true})";
}

#endif
