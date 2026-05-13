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
#include <QString>
#include <QStringList>
#include <QVariant>
#include <QVariantList>

#include <memory>
#include <mutex>

namespace {

constexpr const char* kOffersTopic = "/atomic-swaps/1/offers/json";
constexpr qsizetype kMaxEncodedOfferPayloadChars = 96 * 1024;
constexpr qsizetype kMaxOfferPayloadBytes = 64 * 1024;
constexpr qsizetype kMaxCachedOffers = 256;

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
        if (data.at(1).toString() != QString::fromUtf8(kOffersTopic)) return;

        const QString encodedPayload = data.at(2).toString();
        if (encodedPayload.size() > kMaxEncodedOfferPayloadChars) return;
        const QByteArray decoded = QByteArray::fromBase64(encodedPayload.toUtf8());
        if (decoded.size() > kMaxOfferPayloadBytes) return;
        const auto doc = QJsonDocument::fromJson(decoded);
        if (!doc.isObject()) return;

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

} // namespace

void swapDeliverySetRuntimeLogosAPI(void* api)
{
    DeliveryState& s = state();
    std::lock_guard<std::mutex> opLock(s.operationMutex);
    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    s.api = static_cast<LogosAPI*>(api);
    s.modules = s.api ? std::make_shared<LogosModules>(s.api) : nullptr;
    s.nodeCreated = false;
    s.started = false;
    s.subscribed = false;
    s.connectionStatus.clear();
    s.lastError.clear();
    s.offers = QJsonArray{};
    if (s.modules) {
        wireEventsLocked(s);
    }
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
    }

    return R"({"ok":true,"method":"messagingShutdown","backend":"delivery_module"})";
}

std::string swapDeliveryMessagingStatus()
{
    DeliveryState& s = state();
    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    QJsonObject status{
        {QStringLiteral("ok"), true},
        {QStringLiteral("method"), QStringLiteral("messagingStatus")},
        {QStringLiteral("backend"), QStringLiteral("delivery_module")},
        {QStringLiteral("connected"), s.started},
        {QStringLiteral("peer_count"), 0},
        {QStringLiteral("connection_status"), s.connectionStatus}
    };
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

#else

void swapDeliverySetRuntimeLogosAPI(void*) {}

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
    return R"({"ok":true,"method":"messagingStatus","backend":"delivery_module","connected":false,"peer_count":0,"unavailable":true})";
}

std::string swapDeliveryPublishOffer(const std::string&)
{
    return jsonError("messaging not initialized - call messagingInit first");
}

std::string swapDeliveryFetchOffers()
{
    return R"({"ok":true,"method":"fetchOffers","backend":"delivery_module","offers":[],"unavailable":true})";
}

#endif
