#include "swap_delivery_adapter.h"

#include <cmath>
#include <limits>
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

// Sum the fleet's live relay-peer count out of a delivery_module
// getNodeInfo("Metrics") body (a Prometheus /metrics text exposition).
//
// Source-verified metric choice (delivery_module v0.2.0's pinned lib,
// logos_delivery/waku/node/peer_manager/peer_manager.nim):
//
//   declarePublicGauge logos_delivery_connected_peers,
//     "Number of physical connections per direction and protocol",
//     labels = ["direction", "protocol"]                              (:40-42)
//
// It is set every metrics heartbeat to the current physical connection count
// per (direction, protocol) — logos_delivery_connected_peers.set(
//   protoConnsIn.len, [In, proto]) / (protoConnsOut.len, [Out, proto]) at
// :896-902. We sum only the series whose protocol label is the Waku relay
// codec ("/vac/waku/relay/2.0.0", logos_delivery/waku/waku_core/codecs.nim:2)
// because relay/gossipsub is what carries offers in Core mode; In and Out
// partition the connected peers (a connection is inbound xor outbound), so the
// In+Out sum is the distinct relay-peer count — and it is exactly 0 iff the
// node is meshed with nobody, which is the fleet-isolation failure this probe
// exists to surface. (We do NOT use libp2p's own connmanager gauge: libp2p is
// not vendored in the pinned lib, so its metric name is not source-citable —
// and waku_metrics.nim:30-31 notes "libp2p_pubsub_peers is not public".)
//
// Chosen over the alternatives in the same registry: total_unique_peers is a
// monotonic inc() counter (never decremented, so useless as current state);
// peer_store_size counts every known peer including disconnected ones (so it
// would hide isolation); the base logos_delivery_connected_peers summed across
// ALL protocols would count a peer once per protocol it speaks (an overcount
// the UI would render as inflated "fleet peers").
//
// NodeInfoId.Metrics returns defaultRegistry.toText()
// (logos_delivery/waku/factory/waku_state_info.nim) — standard Prometheus text:
// `# HELP`/`# TYPE` comment lines, then `name{label="v",...} value [ts]` sample
// lines. Returns the summed count (>= 0) when at least one relay
// connected-peers series is present, or -1 (unknown) when the family is absent
// (e.g. the very first heartbeat has not run yet) or the body is not parseable
// metrics text. Deliberately Qt-free (like swapDeliveryEthAmountToWei's
// fallback twin) so it compiles identically in the header-less test build and
// stays a pure, unit-testable function.
int swapDeliveryParsePeerCountFromMetrics(const std::string& metricsText)
{
    static const std::string kMetric = "logos_delivery_connected_peers";

    long long total = 0;
    bool found = false;

    std::size_t pos = 0;
    const std::size_t n = metricsText.size();
    while (pos < n) {
        std::size_t eol = metricsText.find('\n', pos);
        if (eol == std::string::npos) {
            eol = n;
        }
        std::string line = metricsText.substr(pos, eol - pos);
        pos = eol + 1;

        // Trim leading blanks; skip empty and `#` HELP/TYPE comment lines.
        const auto ls = line.find_first_not_of(" \t\r");
        if (ls == std::string::npos || line[ls] == '#') {
            continue;
        }
        line = line.substr(ls);

        // Metric name = everything up to the label brace or a blank. Match the
        // family name EXACTLY: "logos_delivery_connected_peers_per_shard" and
        // "logos_delivery_streams_peers" (also protocol-labelled) must not be
        // mistaken for it.
        const auto nameEnd = line.find_first_of("{ \t");
        if (nameEnd == std::string::npos || line[nameEnd] != '{') {
            continue; // this gauge is always label-qualified
        }
        if (line.compare(0, nameEnd, kMetric) != 0) {
            continue;
        }

        // Walk the label block, honouring quoted strings, to its closing `}`.
        std::size_t i = nameEnd + 1;
        bool inString = false;
        bool escaped = false;
        std::size_t labelEnd = std::string::npos;
        for (; i < line.size(); ++i) {
            const char c = line[i];
            if (inString) {
                if (escaped) {
                    escaped = false;
                } else if (c == '\\') {
                    escaped = true;
                } else if (c == '"') {
                    inString = false;
                }
            } else if (c == '"') {
                inString = true;
            } else if (c == '}') {
                labelEnd = i;
                break;
            }
        }
        if (labelEnd == std::string::npos) {
            continue;
        }
        const std::string labels = line.substr(nameEnd + 1, labelEnd - (nameEnd + 1));

        // Keep only the relay-protocol series (see the codec citation above).
        const std::string protoKey = "protocol=\"";
        const auto pk = labels.find(protoKey);
        if (pk == std::string::npos) {
            continue;
        }
        const auto protoStart = pk + protoKey.size();
        const auto protoEnd = labels.find('"', protoStart);
        if (protoEnd == std::string::npos) {
            continue;
        }
        if (labels.compare(protoStart, protoEnd - protoStart,
                           "/vac/waku/relay/2.0.0")
            != 0) {
            continue;
        }

        // Value = the first whitespace-delimited token after `}` (an optional
        // trailing timestamp, if any, is ignored).
        const auto vs = line.find_first_not_of(" \t", labelEnd + 1);
        if (vs == std::string::npos) {
            continue;
        }
        const auto ve = line.find_first_of(" \t", vs);
        const std::string valueStr =
            line.substr(vs, ve == std::string::npos ? std::string::npos : ve - vs);

        try {
            std::size_t consumed = 0;
            const double value = std::stod(valueStr, &consumed);
            if (consumed != valueStr.size() || !std::isfinite(value) || value < 0) {
                continue; // trailing junk / NaN / negative → not a clean gauge
            }
            total += static_cast<long long>(value + 0.5);
            found = true;
        } catch (...) {
            continue;
        }
    }

    if (!found) {
        return -1;
    }
    if (total > static_cast<long long>(std::numeric_limits<int>::max())) {
        return std::numeric_limits<int>::max();
    }
    return static_cast<int>(total);
}

// logos.dev fleet Waku network parameters (2026-08 migration). The fleet was
// redeployed from cluster 2 to cluster 3 during the Aug-7/8 LEZ/delivery
// upgrade window; see PR #117 for the matching offer-publisher change. The
// shard count is unchanged — the cluster-2 preset already ran 8 autoshards and
// the cluster-3 preset keeps that — so only the cluster id needs overriding.
constexpr int kLogosDevClusterId = 3;

// Only QtCore is needed to build the createNode config JSON — not the logos
// SDK — so this is guarded on QtCore rather than the SDK headers below. That
// keeps it compiled (and therefore unit-tested) in the header-less test build,
// which links QtCore, so the tests exercise the real shipping code path rather
// than a hand-maintained twin. Self-contained (own parse, no SDK helpers) so
// it does not depend on anything inside the SDK #if branch.
#if __has_include(<QtCore/QJsonDocument>)
#include <QtCore/QByteArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonValue>
#include <QtCore/QMetaType>
#include <QtCore/QRegularExpression>
#include <QtCore/QString>
#include <QtCore/QVariant>

// Builds the JSON handed to delivery_module.createNode(). Exposed (external
// linkage) so the swap-module unit tests can assert the fleet cluster override
// travels in the config.
QString swapDeliveryConfigJson(const std::string& configJson)
{
    const QJsonDocument doc = QJsonDocument::fromJson(QByteArray::fromStdString(configJson));
    const QJsonObject input = doc.isObject() ? doc.object() : QJsonObject{};

    const QJsonValue delivery = input.value(QStringLiteral("delivery"));
    if (delivery.isObject()) {
        // Explicit full delivery config: the caller owns every key, so respect
        // it verbatim (no fleet override injected).
        return QString::fromUtf8(QJsonDocument(delivery.toObject()).toJson(QJsonDocument::Compact));
    }

    const QString preset = input.value(QStringLiteral("preset")).toString(QStringLiteral("logos.dev"));

    QJsonObject cfg{
        {QStringLiteral("logLevel"), input.value(QStringLiteral("logLevel")).toString(QStringLiteral("INFO"))},
        {QStringLiteral("mode"), input.value(QStringLiteral("mode")).toString(QStringLiteral("Core"))},
        {QStringLiteral("preset"), preset}
    };
    if (input.contains(QStringLiteral("portsShift"))) {
        cfg.insert(QStringLiteral("portsShift"), input.value(QStringLiteral("portsShift")));
    }

    // Force the logos.dev fleet onto Waku cluster 3 (see kLogosDevClusterId).
    // The preset still resolves to cluster 2, and a node left there dials the
    // fleet but has every subscribe/lightpush rejected — meshing with 0 peers,
    // so no offers arrive.
    //
    // Emit ONLY the flat `clusterId` — the shape delivery_module v0.2.0's
    // SOURCE honours (READMEs have lied before; both claims below are
    // source-verified):
    //
    //  * v0.2.0 (bundled logos-delivery f8b03659): the flat blob parses via
    //    parseFlatConf (logos_delivery/api/conf/logos_delivery_conf_json.nim:
    //    58-100, reached at :148-155) and the builder merge order makes the
    //    EXPLICIT clusterId win — the preset only fills unset fields
    //    (checkSetPresetValueToField + applyNetworkPresetConf,
    //    logos_delivery/waku/factory/conf_builder/waku_conf_builder.nim:
    //    355-389). Result: cluster 3 with the preset's entry nodes and 8
    //    autoshards, i.e. pubsub topics /waku/2/rs/3/<shard>.
    //    Do NOT move clusterId into `messagingOverrides`: that object is a
    //    MessagingClientConf (reliability layer), not a WakuNodeConf, so a
    //    clusterId there is REJECTED — and the layered parser also rejects
    //    mixing messagingOverrides with bare top-level keys like portsShift
    //    (logos_delivery_conf_json.nim:102-188).
    //
    //  * v0.1.x (bundled logos-delivery 509c8755) CANNOT be steered to
    //    cluster 3: it parses the same flat clusterId, then applyNetworkConf
    //    unconditionally overwrites it with the preset's cluster 2
    //    (waku/factory/conf_builder/waku_conf_builder.nim:313-324; warn
    //    "Cluster id was provided alongside a network conf" used=2
    //    discarded=3). Dropping the preset to dodge the stomp forfeits the
    //    fleet entry-node wiring, so v0.1.x is unsupportable against the
    //    migrated fleet — delivery_module >= 0.2.0 is REQUIRED (the hint copy
    //    in swapDeliveryMessagingStatus says so too).
    //
    // createNode still REJECTS unrecognised keys in both versions (v0.1.1
    // fails with "Unrecognized configuration option(s) found: …" → "Failed to
    // create Delivery context", confirmed against the owner's live logs), so
    // nothing beyond recognised flat keys may be emitted; the shard count
    // needs no override (the preset already runs 8 autoshards). Applied only
    // to the logos.dev preset, and never when the caller pinned its own
    // clusterId, so a custom or `twn` config is never clobbered.
    if (preset == QStringLiteral("logos.dev") && !input.contains(QStringLiteral("clusterId"))) {
        cfg.insert(QStringLiteral("clusterId"), kLogosDevClusterId);
    } else if (input.contains(QStringLiteral("clusterId"))) {
        // Caller pinned a cluster explicitly — pass it through untouched.
        cfg.insert(QStringLiteral("clusterId"), input.value(QStringLiteral("clusterId")));
    }
    return QString::fromUtf8(QJsonDocument(cfg).toJson(QJsonDocument::Compact));
}

// Decodes the `payload` argument of delivery_module's `messageReceived` event
// into raw message bytes. Exposed (external linkage) so the swap-module unit
// tests can pin the contract. Returns an empty QByteArray for any shape it
// does not recognise.
//
// The shape changed between delivery_module versions (live incident
// 2026-08-10: the fleet delivered offers and this adapter silently dropped
// every one of them by parsing the new shape with the old contract):
//
//  * v0.2.0 (typed `logos_events:` codegen) declares
//    `messageReceived(messageHash tstr, contentTopic tstr, payload bstr,
//    timestamp int)` (delivery_module.lidl; delivery_module_plugin.h:278) and
//    emits the RAW payload bytes (delivery_module_plugin.cpp:170). The
//    generated cdylib events sidecar marshals `bstr` as the canonical tagged
//    wire form {"_bytes": "<base64url>"} (logos_protocol.h / logos_codec.h),
//    and every Qt-side hop decodes that back to a **QByteArray**
//    (logos-cpp-sdk logos_json_convert.cpp, nlohmannToQVariant's
//    isTaggedBytes branch; re-encoded/decoded losslessly by
//    qvariantToNlohmann / nlohmannArgsToQVariantList on each transport hop).
//    So the QVariant that arrives here is a QByteArray holding the raw
//    message payload — there is NO base64 layer left to strip.
//
//  * v0.1.x hand-marshalled the event and emitted the payload as a base64
//    **QString** (its delivery_module_plugin.cpp:125,
//    `QString::fromLatin1(payloadBytes.toBase64())`). Kept as a fallback
//    because it costs three lines — but v0.1.x cannot reach the migrated
//    fleet anyway (see swapDeliveryConfigJson), so it is strictly legacy.
//    Decoding is strict (AbortOnBase64DecodingErrors): arbitrary text —
//    including raw JSON that would "decode" into garbage under Qt's default
//    lenient mode — is rejected instead of silently corrupted.
QByteArray swapDeliveryDecodeEventPayload(const QVariant& payloadArg)
{
    if (payloadArg.userType() == QMetaType::QByteArray) {
        // delivery_module >= 0.2.0: raw payload bytes.
        return payloadArg.toByteArray();
    }
    if (payloadArg.userType() == QMetaType::QString) {
        // delivery_module v0.1.x: base64 text.
        const auto decoded = QByteArray::fromBase64Encoding(
            payloadArg.toString().toUtf8(),
            QByteArray::Base64Encoding | QByteArray::AbortOnBase64DecodingErrors);
        if (decoded) {
            return *decoded;
        }
    }
    return {};
}

// Strict well-formedness check for a received offer, BEYOND mere field
// presence (hasOfferCoreFields). A malformed offer is dropped on ARRIVAL so it
// never reaches the offer cache or the board:
//
//  * maker_eth_address / eth_htlc_address must be a 20-byte hex address
//    (0x-optional, 40 hex chars);
//  * lez_htlc_program_id must be a 32-byte hex program id (64 hex chars);
//  * lez_amount / eth_amount must parse as a positive, finite number;
//  * lez_timelock / eth_timelock must parse as a positive integer (absolute
//    unix seconds).
//
// The timelock rule is also the NaN/zero-timelock guard: an offer whose
// timelock never resolves to a real future second would render permanently
// "expired" on the board yet never satisfy its prune guard — un-prunable spam.
// Rejecting it at the source closes that at the earliest point. This does NOT
// check the swap VENUE (eth_htlc_address / lez_htlc_program_id equality to the
// app's canonical pin): a well-formed but non-canonical offer is deliberately
// left to travel to the UI, which ghosts it (visible "blocked — unsafe" row)
// while the accept-time venue check remains the true gate. Exposed (external
// linkage, QtCore-only) so the swap-module unit tests exercise the exact
// contract the messageReceived handler relies on.
bool swapDeliveryOfferIsWellFormed(const QJsonObject& offer)
{
    static const QRegularExpression kAddr(
        QStringLiteral("\\A(0x)?[0-9a-fA-F]{40}\\z"));
    static const QRegularExpression kProgram(
        QStringLiteral("\\A(0x)?[0-9a-fA-F]{64}\\z"));

    const auto strOf = [&](const char* key) {
        return offer.value(QLatin1String(key)).toVariant().toString().trimmed();
    };

    if (!kAddr.match(strOf("maker_eth_address")).hasMatch()) {
        return false;
    }
    if (!kAddr.match(strOf("eth_htlc_address")).hasMatch()) {
        return false;
    }
    if (!kProgram.match(strOf("lez_htlc_program_id")).hasMatch()) {
        return false;
    }

    const auto positiveNumber = [&](const char* key) {
        bool ok = false;
        const double v = strOf(key).toDouble(&ok);
        return ok && std::isfinite(v) && v > 0.0;
    };
    if (!positiveNumber("lez_amount") || !positiveNumber("eth_amount")) {
        return false;
    }

    const auto positiveInteger = [&](const char* key) {
        bool ok = false;
        const long long v = strOf(key).toLongLong(&ok);
        return ok && v > 0;
    };
    if (!positiveInteger("lez_timelock") || !positiveInteger("eth_timelock")) {
        return false;
    }

    return true;
}
#endif // __has_include(<QtCore/QJsonDocument>)

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

#include <cstdio>
#include <memory>
#include <mutex>
#include <unordered_map>

namespace {

constexpr const char* kOffersTopic = "/atomic-swaps/1/offers/json";
constexpr qsizetype kMaxOfferPayloadBytes = 64 * 1024;
constexpr qsizetype kMaxCachedOffers = 256;
// Per-maker fairness cap on the offer cache. A spammer rotating hashlocks
// under a single maker address cannot grow the cache unbounded nor bury
// honest makers past this many live offers; the oldest offer from that maker
// is evicted once the cap is reached. Well below kMaxCachedOffers so many
// distinct honest makers still fit.
constexpr int kMaxCachedOffersPerMaker = 8;
constexpr qsizetype kMaxCachedSwapEventsPerSwap = 32;
constexpr qsizetype kMaxTrackedSwaps = 64;

// Fleet peer probing. messagingStatus() is polled every 2s by swap_ui; the
// remote getNodeInfo("Metrics") round trip is throttled to this interval so
// status stays cheap while the peer count stays fresh enough to notice
// isolation.
constexpr qint64 kPeerProbeIntervalMs = 5000;
// After the probe is flagged unsupported (an out-of-date delivery_module
// without the node-info API), retry only occasionally — every poll would just
// burn a CALLBACK_TIMEOUT wait each time.
constexpr qint64 kPeerProbeUnsupportedRetryMs = 60 * 1000;
// Consecutive Metrics-call failures before concluding the installed
// delivery_module simply lacks the node-info API (as opposed to a transient
// hiccup). Only a FAILED getNodeInfo("Metrics") counts here — a call that
// succeeds but has not published a relay-peer series yet is "unknown", not a
// failure (see probeFleetPeers).
constexpr int kPeerProbeFailureLimit = 3;
// The peer-count source, logos_delivery_connected_peers, is refreshed only
// once per delivery-node metrics heartbeat — LogAndMetricsInterval = 5 minutes
// (peer_manager.nim:83, verified in the pinned lib). So a freshly started node
// that is still dialing when the first heartbeat samples it can read 0 relay
// peers and stay frozen at 0 until the next heartbeat up to ~5 min later. To
// avoid alarming a healthy-but-still-connecting node, the zero-peer isolation
// hint only fires once a confirmed 0 has outlived one full refresh cycle plus
// a dialing allowance. A genuinely isolated node (stale module pinned to the
// dead cluster 2) reads 0 at every heartbeat, so it still alarms — just after
// this grace rather than within seconds.
constexpr qint64 kPeerMetricsRefreshMs = 5 * 60 * 1000;
constexpr qint64 kZeroPeerAlarmGraceMs = kPeerMetricsRefreshMs + 45 * 1000;

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
    // Failures of the one-shot Version lookup, counted separately so a
    // module that answers peer counts but not Version stops being asked
    // after kPeerProbeFailureLimit attempts instead of on every probe.
    int versionProbeFailures = 0;
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
        const bool isOffersTopic = contentTopic == QString::fromUtf8(kOffersTopic);
        // v0.2.0 delivers the payload as raw bytes in a QByteArray; v0.1.x
        // delivered a base64 QString. swapDeliveryDecodeEventPayload owns
        // that (unit-tested) contract. Parsing the QByteArray shape with the
        // old base64-QString code was the 2026-08-10 empty-offer-board bug.
        const QByteArray decoded = swapDeliveryDecodeEventPayload(data.at(2));
        if (decoded.isEmpty() || decoded.size() > kMaxOfferPayloadBytes) {
            if (isOffersTopic) {
                fprintf(stderr,
                        "SwapDeliveryAdapter: dropped offers-topic payload "
                        "(undecodable or oversized payload argument)\n");
            }
            return;
        }
        const auto doc = QJsonDocument::fromJson(decoded);
        if (!doc.isObject()) {
            if (isOffersTopic) {
                fprintf(stderr,
                        "SwapDeliveryAdapter: dropped offers-topic payload "
                        "(payload is not a JSON object)\n");
            }
            return;
        }

        if (isOffersTopic) {
            QJsonObject offer = filteredOfferFields(doc.object());
            if (!hasOfferCoreFields(offer)) {
                fprintf(stderr,
                        "SwapDeliveryAdapter: dropped offers-topic payload "
                        "(missing required offer fields)\n");
                return;
            }
            // Receive-side malformed filter (defense-in-depth, review P1s:
            // offer type-validation + NaN-timelock). A bad-hex address, a
            // non-numeric/non-positive amount, or a NaN/negative/zero timelock
            // is dropped here so it never reaches the cache or the board.
            // Venue (canonical eth_htlc_address / lez_htlc_program_id) is NOT
            // checked here — a well-formed but non-canonical offer travels on
            // to the UI, which ghosts it; the accept-time check is the gate.
            if (!swapDeliveryOfferIsWellFormed(offer)) {
                fprintf(stderr,
                        "SwapDeliveryAdapter: dropped offers-topic payload "
                        "(malformed offer fields)\n");
                return;
            }
            offer.insert(QStringLiteral("message_hash"), data.at(0).toString());
            offer.insert(QStringLiteral("timestamp_ms"), QDateTime::currentMSecsSinceEpoch());

            DeliveryState& st = state();
            std::lock_guard<std::recursive_mutex> lock(st.mutex);
            while (st.offers.size() >= kMaxCachedOffers) {
                st.offers.removeAt(0);
            }
            // Per-maker fairness cap (review P1: spam-cap). Evict this maker's
            // oldest cached offer once it already holds kMaxCachedOffersPerMaker
            // — a spammer rotating hashlocks under one address cannot bury
            // honest makers.
            const QString makerKey = offer.value(QStringLiteral("maker_eth_address"))
                                         .toString().trimmed().toLower();
            if (!makerKey.isEmpty()) {
                const auto makerCount = [&]() {
                    int c = 0;
                    for (const QJsonValue& v : st.offers) {
                        if (v.toObject().value(QStringLiteral("maker_eth_address"))
                                .toString().trimmed().toLower() == makerKey) {
                            ++c;
                        }
                    }
                    return c;
                };
                while (makerCount() >= kMaxCachedOffersPerMaker) {
                    bool removed = false;
                    for (int i = 0; i < st.offers.size(); ++i) {
                        if (st.offers.at(i).toObject()
                                .value(QStringLiteral("maker_eth_address"))
                                .toString().trimmed().toLower() == makerKey) {
                            st.offers.removeAt(i);
                            removed = true;
                            break;
                        }
                    }
                    if (!removed) {
                        break;
                    }
                }
            }
            st.offers.append(offer);
            // Stable end-to-end reception marker: the basecamp-ui-smoke CI
            // lane greps the captured host log for this exact substring to
            // prove a real fleet offer traveled delivery_module ->
            // adapter -> offer cache (tests/basecamp-ui-smoke.mjs). Change
            // the wording there too if it ever changes here.
            fprintf(stderr,
                    "SwapDeliveryAdapter: offer cached from delivery hash=%s\n",
                    offer.value(QStringLiteral("message_hash"))
                        .toString().toUtf8().constData());
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
// delivery_module cannot be rejected at load time. At runtime we read the
// node's own Prometheus registry via getNodeInfo("Metrics") — a real
// NodeInfoId in the pinned lib (logos_delivery/waku/factory/
// waku_state_info.nim, which returns defaultRegistry.toText()) — and sum the
// relay connected-peers gauge out of it (swapDeliveryParsePeerCountFromMetrics).
// So:
//  - a healthy module answers Metrics and the relay-peer sum > 0 → connected;
//  - a healthy-but-isolated module answers Metrics but the relay-peer sum is 0
//    → zero-peer fleet isolation becomes visible instead of hiding behind
//    "Delivery connected";
//  - a months-old module that predates the node-info API fails the Metrics
//    call entirely → flagged as out of date.
//
// (History: this probe used to call getNodeInfo("relayPeerCount") with a
// "peerCount" fallback. Those IDs never existed in ANY delivery_module — the
// real NodeInfoId vocabulary is Version/Metrics/MyMultiaddresses/MyENR/
// MyPeerId/MyBoundPorts/MyMixPubKey/MaxMessageSize — so parseEnum rejected them
// and every probe failed. That tripped kPeerProbeFailureLimit on every healthy
// node, pinning the UI to a permanent "Delivery degraded" banner even while
// offers arrived. The getNodeInfo *method* was real; only the requested IDs
// were fictional.)
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
        needVersion = s.deliveryVersion.isEmpty()
            && s.versionProbeFailures < kPeerProbeFailureLimit;
    }

    // Status polling must stay cheap: never queue behind a long-running
    // delivery operation (createNode can take seconds). If an operation is
    // in flight, skip this probe and keep the cached value. The lock IS held
    // across the getNodeInfo calls below — deliberately: every other call
    // through the shared LogosAPI client serializes on this mutex and the
    // client's thread-safety under concurrent invocation is not guaranteed,
    // so copy-then-call-unlocked is not safe here. try_lock keeps the worst
    // case for the status poll at "skip one probe", never a wait. (The
    // generated synchronous getNodeInfo takes no per-call timeout to
    // shorten; the throttle + failure backoff bound repeated stalls.)
    std::unique_lock<std::mutex> opLock(s.operationMutex, std::try_to_lock);
    if (!opLock.owns_lock()) {
        return;
    }

    // Read the node's Prometheus registry and sum the relay connected-peers
    // gauge out of it. A SUCCESSFUL Metrics call proves the module supports the
    // node-info API (so it is not "out of date"), even before it has published
    // a relay-peer series — that early window reads as count -1 (unknown), not
    // as a probe failure.
    bool metricsAnswered = false;
    int count = -1;
    const LogosResult metrics =
        modules->delivery_module.getNodeInfo(QStringLiteral("Metrics"));
    if (metrics.success) {
        metricsAnswered = true;
        count = swapDeliveryParsePeerCountFromMetrics(metrics.getString().toStdString());
    }

    QString version;
    if (metricsAnswered && needVersion) {
        const LogosResult v = modules->delivery_module.getNodeInfo(QStringLiteral("Version"));
        if (v.success) {
            version = v.getString().trimmed();
        }
    }

    std::lock_guard<std::recursive_mutex> lock(s.mutex);
    if (metricsAnswered) {
        // The module answered — it has the node-info API. count may still be -1
        // (relay-peer series not published yet), which surfaces as "peer count
        // unknown", not as fleet isolation.
        s.fleetPeerCount = count;
        s.peerProbeFailures = 0;
        s.peerInfoUnsupported = false;
        if (needVersion) {
            if (!version.isEmpty()) {
                if (s.deliveryVersion.isEmpty()) {
                    s.deliveryVersion = version;
                }
                s.versionProbeFailures = 0;
            } else {
                ++s.versionProbeFailures;
            }
        }
    } else {
        // getNodeInfo("Metrics") itself failed — treat as a module predating
        // the node-info API once it has failed kPeerProbeFailureLimit times.
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
    s.versionProbeFailures = 0;
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
        LogosResult created = modules->delivery_module.createNode(swapDeliveryConfigJson(configJson));
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
            // State the observable fact, then the known floor: the migrated
            // logos.dev fleet needs delivery_module >= 0.2.0 (older builds are
            // preset-pinned to the dead cluster 2 — see swapDeliveryConfigJson).
            hint = QStringLiteral(
                "delivery_module did not answer its node-info metrics query — "
                "offers may not be arriving. The logos.dev fleet requires "
                "delivery_module >= 0.2.0; install the update from the module "
                "manager and restart Basecamp.");
        } else if (s.fleetPeerCount == 0
                   && QDateTime::currentMSecsSinceEpoch() - s.startedAtMs
                          > kZeroPeerAlarmGraceMs) {
            // Grace-gated: 0 peers seconds after start is normal dialing,
            // not isolation. The dominant cause is a pre-0.2.0
            // delivery_module: its preset stomps the cluster-3 override
            // (source-verified — see swapDeliveryConfigJson), leaving the
            // node meshed with nobody on cluster 2.
            hint = QStringLiteral(
                "connected to 0 fleet peers, so offers cannot arrive. The "
                "logos.dev fleet (Waku cluster 3) requires delivery_module "
                ">= 0.2.0 — older builds are pinned to cluster 2 by their "
                "preset. Install the delivery_module update from the module "
                "manager and restart Basecamp.");
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
