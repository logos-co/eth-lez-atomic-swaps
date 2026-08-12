#include <logos_test.h>
#include "../src/swap_impl.h"

#include <QByteArray>
#include <QCoreApplication>
#include <QJsonDocument>
#include <QJsonObject>
#include <QString>
#include <QVariant>

#include <chrono>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

std::string swapDeliveryEthAmountToWei(const std::string& ethAmount);
std::string swapDeliveryBuildOfferRequest(long long unixTimeSecs);
int swapDeliveryParsePeerCountFromMetrics(const std::string& metricsText);
QString swapDeliveryConfigJson(const std::string& configJson);
QByteArray swapDeliveryDecodeEventPayload(const QVariant& payloadArg);
bool swapDeliveryOfferIsWellFormed(const QJsonObject& offer);

extern "C" {
void mock_swap_ffi_reset();
void mock_swap_ffi_set_load_env_error(bool enabled);
int mock_swap_ffi_load_env_calls();
int mock_swap_ffi_fetch_balances_calls();
int mock_swap_ffi_free_string_calls();
const char* mock_swap_ffi_last_load_env_path();
const char* mock_swap_ffi_last_fetch_balances_config();
const char* mock_swap_ffi_call_sequence();
}

namespace {

bool waitForContains(SwapImpl& impl, const std::string& jobId, const std::string& needle)
{
    for (int i = 0; i < 50; ++i) {
        if (impl.jobStatus(jobId).find(needle) != std::string::npos) {
            return true;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    return false;
}

std::string extractJobId(const std::string& json)
{
    const std::string key = R"("job_id":")";
    const auto start = json.find(key);
    if (start == std::string::npos) {
        return {};
    }
    const auto valueStart = start + key.size();
    const auto valueEnd = json.find('"', valueStart);
    if (valueEnd == std::string::npos) {
        return {};
    }
    return json.substr(valueStart, valueEnd - valueStart);
}

bool waitForEvent(std::mutex& mutex,
                  const std::vector<std::string>& events,
                  const std::string& firstNeedle,
                  const std::string& secondNeedle = {})
{
    for (int i = 0; i < 50; ++i) {
        QCoreApplication::processEvents();
        {
            std::lock_guard<std::mutex> lock(mutex);
            for (const auto& event : events) {
                if (event.find(firstNeedle) != std::string::npos
                    && (secondNeedle.empty() || event.find(secondNeedle) != std::string::npos)) {
                    return true;
                }
            }
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    return false;
}

} // namespace

LOGOS_TEST(swap_impl_can_be_constructed) {
    SwapImpl impl;
    (void)impl;
}

LOGOS_TEST(stop_maker_loop_is_safe_when_idle) {
    SwapImpl impl;
    impl.stopMakerLoop();
}

LOGOS_TEST(fetch_balances_returns_ffi_json) {
    SwapImpl impl;
    LOGOS_ASSERT_CONTAINS(impl.fetchBalances("{}"), R"("method":"fetchBalances")");
}

LOGOS_TEST(fetch_balances_from_env_loads_config_internally) {
    mock_swap_ffi_reset();
    SwapImpl impl;

    const auto result = impl.fetchBalancesFromEnv("/tmp/swap.env");

    LOGOS_ASSERT_CONTAINS(result, R"("method":"fetchBalances")");
    LOGOS_ASSERT_EQ(mock_swap_ffi_load_env_calls(), 1);
    LOGOS_ASSERT_EQ(mock_swap_ffi_fetch_balances_calls(), 1);
    LOGOS_ASSERT_EQ(std::string(mock_swap_ffi_call_sequence()), std::string("loadEnv>fetchBalances"));
    LOGOS_ASSERT_EQ(std::string(mock_swap_ffi_last_load_env_path()), std::string("/tmp/swap.env"));
    LOGOS_ASSERT_CONTAINS(std::string(mock_swap_ffi_last_fetch_balances_config()), R"("eth_rpc_url":"ws://127.0.0.1:8545")");
    LOGOS_ASSERT_EQ(mock_swap_ffi_free_string_calls(), 2);
}

LOGOS_TEST(fetch_balances_from_env_returns_load_env_error_without_fetching) {
    mock_swap_ffi_reset();
    mock_swap_ffi_set_load_env_error(true);
    SwapImpl impl;

    const auto result = impl.fetchBalancesFromEnv("/tmp/bad.env");

    LOGOS_ASSERT_CONTAINS(result, R"("error":"forced load env failure")");
    LOGOS_ASSERT_EQ(mock_swap_ffi_load_env_calls(), 1);
    LOGOS_ASSERT_EQ(mock_swap_ffi_fetch_balances_calls(), 0);
    LOGOS_ASSERT_EQ(std::string(mock_swap_ffi_call_sequence()), std::string("loadEnv"));
    LOGOS_ASSERT_EQ(mock_swap_ffi_free_string_calls(), 1);
}

LOGOS_TEST(messaging_status_uses_delivery_backend_shape) {
    SwapImpl impl;
    const auto status = impl.messagingStatus();
    LOGOS_ASSERT_CONTAINS(status, R"("method":"messagingStatus")");
    LOGOS_ASSERT_CONTAINS(status, R"("backend":"delivery_module")");
    LOGOS_ASSERT_CONTAINS(status, R"("connected":false)");
    // Before any successful peer probe the count is a placeholder, and the
    // status must say so — the UI treats a KNOWN zero as fleet isolation, so
    // an unknown one must never masquerade as a confirmed reading.
    LOGOS_ASSERT_CONTAINS(status, R"("peer_count_known":false)");
}

// The fleet peer count is read from a delivery_module getNodeInfo("Metrics")
// body — Prometheus /metrics text. The parser sums the relay-protocol series
// of the source-verified logos_delivery_connected_peers gauge (In+Out =
// distinct relay peers; relay/gossipsub carries offers in Core mode) and
// refuses to invent a count from anything else (returning -1 = unknown, never
// a fake 0). A realistic registry body carries # HELP/# TYPE comments,
// unrelated metric families, and sibling families whose names share the
// prefix — none of which may be mistaken for the relay peer count.
namespace {
const char* const kMetricsSample =
    "# HELP logos_delivery_node_messages Number of messages\n"
    "# TYPE logos_delivery_node_messages counter\n"
    "logos_delivery_node_messages_total 41.0\n"
    "# HELP logos_delivery_connected_peers Number of physical connections per direction and protocol\n"
    "# TYPE logos_delivery_connected_peers gauge\n"
    "logos_delivery_connected_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\"} 3.0\n"
    "logos_delivery_connected_peers{direction=\"Out\",protocol=\"/vac/waku/relay/2.0.0\"} 5.0\n"
    // Non-relay protocol series must NOT be counted as relay peers.
    "logos_delivery_connected_peers{direction=\"Out\",protocol=\"/vac/waku/store/3.0.0\"} 9.0\n"
    // Sibling families that share the prefix / a protocol label must NOT match.
    "logos_delivery_connected_peers_per_shard{shard=\"5\"} 2.0\n"
    "logos_delivery_streams_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\"} 7.0\n"
    "# TYPE logos_delivery_peer_store_size gauge\n"
    "logos_delivery_peer_store_size 40.0\n";
} // namespace

LOGOS_TEST(delivery_metrics_peer_count_sums_relay_connections) {
    // 3 In + 5 Out relay connections = 8 distinct relay peers; the store-proto
    // series, the _per_shard sibling, and streams_peers are all ignored.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(kMetricsSample), 8);
}

LOGOS_TEST(delivery_metrics_peer_count_handles_label_order_and_int_values) {
    // protocol label may precede direction, and nim-metrics may render a bare
    // integer (no ".0") — both must parse.
    const std::string body =
        "logos_delivery_connected_peers{protocol=\"/vac/waku/relay/2.0.0\",direction=\"In\"} 2\n"
        "logos_delivery_connected_peers{protocol=\"/vac/waku/relay/2.0.0\",direction=\"Out\"} 4\n";
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(body), 6);
}

LOGOS_TEST(delivery_metrics_peer_count_zero_when_relay_series_present_but_empty) {
    // A live node meshed with nobody publishes the relay series as 0 — a
    // CONFIRMED zero (the fleet-isolation signal), not "unknown".
    const std::string body =
        "logos_delivery_connected_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\"} 0.0\n"
        "logos_delivery_connected_peers{direction=\"Out\",protocol=\"/vac/waku/relay/2.0.0\"} 0.0\n";
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(body), 0);
}

LOGOS_TEST(delivery_metrics_peer_count_unknown_when_relay_series_absent) {
    // No relay connected-peers series at all (e.g. the metrics heartbeat has
    // not run yet, or only non-relay protocols are connected) → unknown (-1),
    // never a fabricated 0 that would trip the isolation alarm.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(""), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics("   \n\t\n"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics("not prometheus text at all"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_peer_store_size 40.0\n"), -1);
    // Right family, but only non-relay protocols are connected.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_connected_peers{direction=\"In\",protocol=\"/vac/waku/store/3.0.0\"} 9.0\n"),
                    -1);
    // Prefix-sibling families alone must not be read as the relay peer count.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_connected_peers_per_shard{shard=\"5\"} 2.0\n"),
                    -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_streams_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\"} 7.0\n"),
                    -1);
}

LOGOS_TEST(delivery_metrics_peer_count_rejects_garbage_values) {
    // A relay series whose value is not a clean finite non-negative number is
    // not a countable reading — skipped, so a body with only garbage is
    // unknown (-1), not 0.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_connected_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\"} NaN\n"),
                    -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_connected_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\"} 3peers\n"),
                    -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_connected_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\"} -2\n"),
                    -1);
    // A malformed (unterminated) label block yields no value → unknown.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_connected_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\" 3.0\n"),
                    -1);
    // A good relay series alongside a garbage one still counts the good one
    // (Prometheus bodies are line-oriented; one bad line must not poison the
    // whole scrape), and an optional trailing timestamp is ignored.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCountFromMetrics(
                        "logos_delivery_connected_peers{direction=\"In\",protocol=\"/vac/waku/relay/2.0.0\"} 4.0 1699999999000\n"
                        "logos_delivery_connected_peers{direction=\"Out\",protocol=\"/vac/waku/relay/2.0.0\"} bogus\n"),
                    4);
}

LOGOS_TEST(delivery_messaging_requires_runtime_before_init_or_publish) {
    SwapImpl impl;
    LOGOS_ASSERT_CONTAINS(impl.messagingInit("{}"), R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(impl.messagingInit("{}"), "delivery_module runtime");
    LOGOS_ASSERT_CONTAINS(impl.publishOffer("{}"), R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(impl.publishOffer("{}"), "messaging not initialized");
}

// RFQ: the taker's offer-request payload must be exactly the shape the maker's
// rfq.mjs parser expects, and must carry NO taker identity (privacy).
LOGOS_TEST(offer_request_payload_is_anonymous_and_well_formed) {
    LOGOS_ASSERT_EQ(swapDeliveryBuildOfferRequest(1700000000LL),
                    std::string(R"({"v":1,"kind":"offer_request","ts":1700000000})"));
    const auto payload = swapDeliveryBuildOfferRequest(42LL);
    LOGOS_ASSERT(payload.find("account") == std::string::npos);
    LOGOS_ASSERT(payload.find("address") == std::string::npos);
    LOGOS_ASSERT(payload.find("taker") == std::string::npos);
}

LOGOS_TEST(publish_offer_request_requires_runtime) {
    SwapImpl impl;
    LOGOS_ASSERT_CONTAINS(impl.publishOfferRequest(), R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(impl.publishOfferRequest(), "messaging not initialized");
}

LOGOS_TEST(delivery_eth_amount_decimal_normalizes_to_wei) {
    LOGOS_ASSERT_EQ(swapDeliveryEthAmountToWei("0.00000000000000001"), std::string("10"));
}

// The logos.dev fleet migrated Waku cluster 2 -> 3. deliveryConfig must force
// cluster 3 over the still-cluster-2 preset with the flat `clusterId` key ONLY.
// Source-verified against delivery_module v0.2.0 (bundled logos-delivery
// f8b03659): the flat blob parses via parseFlatConf
// (logos_delivery/api/conf/logos_delivery_conf_json.nim:58-100) and the
// EXPLICIT clusterId beats the preset's cluster 2 — presets only fill unset
// fields (checkSetPresetValueToField, logos_delivery/waku/factory/
// conf_builder/waku_conf_builder.nim:355-389) — landing the node on
// /waku/2/rs/3/<shard>. clusterId must NOT move into `messagingOverrides`
// (that is the reliability-layer conf; a clusterId there is rejected, and the
// layered parser also rejects mixing messagingOverrides with bare keys like
// portsShift). createNode REJECTS unrecognised keys in every shipped version —
// v0.1.1 fails createNode with "Unrecognized configuration option(s) found:
// [numShardsInCluster, messagingOverrides]" -> "Failed to create Delivery
// context" — so those keys must never be emitted; the preset already runs 8
// autoshards. Note delivery_module v0.1.x is unsupportable on the migrated
// fleet regardless of emission: its logos-delivery (509c8755) stomps the
// explicit clusterId with the preset's cluster 2 (waku_conf_builder.nim:
// 313-324). See PR #117 and the delivery-context-creation fix.
LOGOS_TEST(delivery_config_forces_logos_dev_cluster_three) {
    const std::string cfg = swapDeliveryConfigJson("{}").toStdString();
    LOGOS_ASSERT_CONTAINS(cfg, R"("preset":"logos.dev")");
    LOGOS_ASSERT_CONTAINS(cfg, R"("clusterId":3)");
    // Source contract: the unrecognised keys that break createNode must never
    // appear in the emitted config.
    LOGOS_ASSERT(cfg.find("numShardsInCluster") == std::string::npos);
    LOGOS_ASSERT(cfg.find("messagingOverrides") == std::string::npos);
}

// A caller that pins its own clusterId owns the network choice: no cluster-3
// override is injected, and no unrecognised keys are added.
LOGOS_TEST(delivery_config_respects_explicit_cluster) {
    const std::string cfg = swapDeliveryConfigJson(R"({"clusterId":2})").toStdString();
    LOGOS_ASSERT_CONTAINS(cfg, R"("clusterId":2)");
    LOGOS_ASSERT(cfg.find(R"("clusterId":3)") == std::string::npos);
    LOGOS_ASSERT(cfg.find("numShardsInCluster") == std::string::npos);
    LOGOS_ASSERT(cfg.find("messagingOverrides") == std::string::npos);
}

// Even if a caller supplies the (unrecognised) numShardsInCluster key, it must
// never be forwarded into the createNode config — createNode would reject it.
LOGOS_TEST(delivery_config_drops_unrecognized_shard_key) {
    const std::string cfg =
        swapDeliveryConfigJson(R"({"clusterId":2,"numShardsInCluster":8})").toStdString();
    LOGOS_ASSERT_CONTAINS(cfg, R"("clusterId":2)");
    LOGOS_ASSERT(cfg.find("numShardsInCluster") == std::string::npos);
    LOGOS_ASSERT(cfg.find("messagingOverrides") == std::string::npos);
}

// A non-logos.dev preset (e.g. the RLN twn network) is never forced onto the
// logos.dev fleet's cluster.
LOGOS_TEST(delivery_config_non_logos_preset_untouched) {
    const std::string cfg = swapDeliveryConfigJson(R"({"preset":"twn"})").toStdString();
    LOGOS_ASSERT_CONTAINS(cfg, R"("preset":"twn")");
    LOGOS_ASSERT(cfg.find(R"("clusterId":3)") == std::string::npos);
    LOGOS_ASSERT(cfg.find("messagingOverrides") == std::string::npos);
}

// An explicit full `delivery` config is passed through verbatim — the caller
// owns every key, so no fleet override is injected.
LOGOS_TEST(delivery_config_explicit_delivery_passthrough) {
    const std::string cfg =
        swapDeliveryConfigJson(R"({"delivery":{"preset":"custom","clusterId":9}})").toStdString();
    LOGOS_ASSERT_CONTAINS(cfg, R"("preset":"custom")");
    LOGOS_ASSERT_CONTAINS(cfg, R"("clusterId":9)");
    LOGOS_ASSERT(cfg.find("messagingOverrides") == std::string::npos);
}

// ── messageReceived payload decoding (delivery_module event contract) ──────
//
// delivery_module v0.2.0 declares `messageReceived(messageHash tstr,
// contentTopic tstr, payload bstr, timestamp int)` via the typed
// `logos_events:` codegen. `bstr` crosses every hop as the canonical tagged
// bytes form ({"_bytes": base64url}, logos_codec.h) and each Qt-side decode
// (logos_json_convert.cpp nlohmannToQVariant) turns it back into a QByteArray
// of RAW payload bytes. v0.1.x hand-marshalled the same event with the
// payload as a base64 QString. The 2026-08-10 live incident: the node
// received and re-relayed real fleet offers, but the adapter parsed the
// v0.2.0 QByteArray with the v0.1.x contract (toString() -> fromBase64())
// and silently dropped every offer — an empty board with a green
// "Delivery connected". These tests pin the verified real shapes.

LOGOS_TEST(delivery_event_payload_bytearray_is_raw_passthrough) {
    // The verified v0.2.0 shape: raw JSON bytes in a QByteArray — no base64
    // layer to strip.
    const QByteArray raw = R"({"hashlock":"ab","lez_amount":"1"})";
    LOGOS_ASSERT_EQ(swapDeliveryDecodeEventPayload(QVariant(raw)).toStdString(),
                    raw.toStdString());
}

LOGOS_TEST(delivery_event_payload_string_is_strict_base64) {
    // The v0.1.x legacy shape: base64 text.
    const QByteArray raw = R"({"hashlock":"ab"})";
    const QVariant encoded(QString::fromLatin1(raw.toBase64()));
    LOGOS_ASSERT_EQ(swapDeliveryDecodeEventPayload(encoded).toStdString(),
                    raw.toStdString());
}

LOGOS_TEST(delivery_event_payload_rejects_unrecognized_shapes) {
    // Raw JSON text inside a QString is NOT valid base64 and must be
    // rejected, not garbage-decoded (Qt's lenient default would happily
    // "decode" it — exactly how the incident stayed silent).
    LOGOS_ASSERT(swapDeliveryDecodeEventPayload(
        QVariant(QStringLiteral(R"({"not":"base64"})"))).isEmpty());
    LOGOS_ASSERT(swapDeliveryDecodeEventPayload(QVariant(42)).isEmpty());
    LOGOS_ASSERT(swapDeliveryDecodeEventPayload(QVariant()).isEmpty());
}

LOGOS_TEST(fetch_offers_preserves_empty_offers_shape_without_runtime) {
    SwapImpl impl;
    const auto offers = impl.fetchOffers();
    LOGOS_ASSERT_CONTAINS(offers, R"("offers":[])");
    LOGOS_ASSERT_CONTAINS(offers, R"("backend":"delivery_module")");
}

// ── receive-side offer well-formedness (feat/safe-offer-board) ─────────────
//
// swapDeliveryOfferIsWellFormed is the malformed-offer drop that runs in the
// messageReceived handler BEFORE an offer is cached. It closes three
// messaging-review P1s at the source: offer type-validation, NaN-timelock
// (un-prunable "expired forever" spam), and non-numeric amounts. It does NOT
// check the swap VENUE — a well-formed but non-canonical offer is deliberately
// left to travel to the UI (which ghosts it); these tests pin that boundary.

namespace {
QJsonObject offerObj(const char* json) {
    return QJsonDocument::fromJson(QByteArray(json)).object();
}
// A canonical, fully well-formed offer as it arrives on the wire: hex
// addresses/program, integer wei eth_amount, decimal lez_amount, absolute
// unix-second timelocks.
const char* kGoodOffer = R"({
    "hashlock":"aa",
    "lez_amount":"150",
    "eth_amount":"200000000000000",
    "maker_eth_address":"0x351B0EA07739FA9F6769213927D7836a790A5FAF",
    "maker_lez_account":"lez1abc",
    "lez_timelock":2000000000,
    "eth_timelock":2000000600,
    "lez_htlc_program_id":"0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20",
    "eth_htlc_address":"0x351B0EA07739FA9F6769213927D7836a790A5FAF"
})";
} // namespace

LOGOS_TEST(offer_wellformed_accepts_canonical_and_non_canonical_venue) {
    // Honest, well-formed offer → accepted.
    LOGOS_ASSERT(swapDeliveryOfferIsWellFormed(offerObj(kGoodOffer)));

    // A DIFFERENT (attacker) but structurally VALID venue is still well-formed
    // — the malformed filter must pass it through so the UI can ghost it.
    QJsonObject nonCanonical = offerObj(kGoodOffer);
    nonCanonical.insert(QStringLiteral("eth_htlc_address"),
                        QStringLiteral("0xdeadBEEFdeadBEEFdeadBEEFdeadBEEFdeadBEEF"));
    LOGOS_ASSERT(swapDeliveryOfferIsWellFormed(nonCanonical));
}

LOGOS_TEST(offer_wellformed_rejects_bad_hex_and_missing_fields) {
    // Non-hex maker address.
    QJsonObject badMaker = offerObj(kGoodOffer);
    badMaker.insert(QStringLiteral("maker_eth_address"), QStringLiteral("not-an-address"));
    LOGOS_ASSERT(!swapDeliveryOfferIsWellFormed(badMaker));

    // Truncated program id (too short).
    QJsonObject badProgram = offerObj(kGoodOffer);
    badProgram.insert(QStringLiteral("lez_htlc_program_id"), QStringLiteral("0102"));
    LOGOS_ASSERT(!swapDeliveryOfferIsWellFormed(badProgram));

    // Missing eth_htlc_address entirely.
    QJsonObject missing = offerObj(kGoodOffer);
    missing.remove(QStringLiteral("eth_htlc_address"));
    LOGOS_ASSERT(!swapDeliveryOfferIsWellFormed(missing));
}

LOGOS_TEST(offer_wellformed_rejects_nan_negative_and_zero_timelocks) {
    // NaN timelock (a string that never parses to a real second) — the
    // un-prunable-spam bug this filter closes.
    QJsonObject nan = offerObj(kGoodOffer);
    nan.insert(QStringLiteral("lez_timelock"), QStringLiteral("NaN"));
    LOGOS_ASSERT(!swapDeliveryOfferIsWellFormed(nan));

    // Zero timelock.
    QJsonObject zero = offerObj(kGoodOffer);
    zero.insert(QStringLiteral("eth_timelock"), 0);
    LOGOS_ASSERT(!swapDeliveryOfferIsWellFormed(zero));

    // Negative timelock.
    QJsonObject neg = offerObj(kGoodOffer);
    neg.insert(QStringLiteral("lez_timelock"), -5);
    LOGOS_ASSERT(!swapDeliveryOfferIsWellFormed(neg));
}

LOGOS_TEST(offer_wellformed_rejects_bad_amounts) {
    // Non-numeric amount.
    QJsonObject junk = offerObj(kGoodOffer);
    junk.insert(QStringLiteral("eth_amount"), QStringLiteral("free-money"));
    LOGOS_ASSERT(!swapDeliveryOfferIsWellFormed(junk));

    // Zero amount.
    QJsonObject zero = offerObj(kGoodOffer);
    zero.insert(QStringLiteral("lez_amount"), QStringLiteral("0"));
    LOGOS_ASSERT(!swapDeliveryOfferIsWellFormed(zero));
}

LOGOS_TEST(run_maker_forwards_progress_events) {
    SwapImpl impl;
    std::string progressData;
    impl.emitEvent = [&](const std::string& name, const std::string& data) {
        if (name == "maker.progress") {
            progressData = data;
        }
    };

    LOGOS_ASSERT_CONTAINS(impl.runMaker("{}", ""), R"("method":"runMaker")");
    LOGOS_ASSERT_CONTAINS(progressData, R"("role":"maker")");
    LOGOS_ASSERT_CONTAINS(progressData, R"("step":"EthLockDetected")");
}

LOGOS_TEST(load_env_returns_config_json) {
    // Pre-existing test-order bug, surfaced while adding the onboarding
    // tests below: fetch_balances_from_env_returns_load_env_error_without_fetching
    // sets the mock's global loadEnvShouldError flag and never clears it, so
    // this test fails with "forced load env failure" whenever the test
    // framework happens to run that one first. Not exercised by this test at
    // all (it doesn't touch the error path), so reset defensively rather than
    // rely on execution order across tests.
    mock_swap_ffi_reset();
    SwapImpl impl;
    const auto config = impl.loadEnv(".env");
    LOGOS_ASSERT_CONTAINS(config, R"("eth_rpc_url")");
    LOGOS_ASSERT_CONTAINS(config, R"("eth_timelock_minutes":"10")");
    LOGOS_ASSERT_CONTAINS(config, R"("lez_timelock_minutes":"5")");
}

LOGOS_TEST(start_maker_job_returns_status_and_finished_event) {
    SwapImpl impl;
    std::mutex mutex;
    std::vector<std::string> events;
    impl.emitEvent = [&](const std::string& name, const std::string& data) {
        std::lock_guard<std::mutex> lock(mutex);
        events.push_back(name + ":" + data);
    };

    const auto started = impl.startMakerJob("{}", "");
    LOGOS_ASSERT_CONTAINS(started, R"("ok":true)");
    LOGOS_ASSERT_CONTAINS(started, R"("role":"maker")");
    const auto jobId = extractJobId(started);
    LOGOS_ASSERT_FALSE(jobId.empty());
    LOGOS_ASSERT(waitForContains(impl, jobId, R"("status":"completed")"));

    LOGOS_ASSERT_TRUE(waitForEvent(mutex, events,
                                   "maker.progress:",
                                   std::string{R"("job_id":")"} + jobId + R"(")"));
    LOGOS_ASSERT_TRUE(waitForEvent(mutex, events, "maker.finished:", "runMaker"));
}

LOGOS_TEST(conflicting_maker_job_is_rejected) {
    SwapImpl impl;
    const auto first = impl.startMakerJob("{}", "");
    const auto jobId = extractJobId(first);
    LOGOS_ASSERT_FALSE(jobId.empty());

    const auto second = impl.startMakerJob("{}", "");
    LOGOS_ASSERT_CONTAINS(second, R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(second, "maker job already running");
    LOGOS_ASSERT(waitForContains(impl, jobId, R"("status":"completed")"));
}

LOGOS_TEST(stop_maker_loop_job_marks_job_and_finishes) {
    SwapImpl impl;
    const auto started = impl.startMakerLoopJob("{}");
    const auto jobId = extractJobId(started);
    LOGOS_ASSERT_FALSE(jobId.empty());

    const auto stopped = impl.stopJob(jobId);
    LOGOS_ASSERT_CONTAINS(stopped, R"("cancel_requested":true)");
    LOGOS_ASSERT(waitForContains(impl, jobId, R"("status":"completed")"));
}

// ── Per-swap coordination (M2 Delivery) wrapper surface ────────────────────

LOGOS_TEST(subscribe_swap_requires_runtime) {
    // The unit-test build links against the no-runtime adapter stub, so the
    // active-branch hashlock validation isn't exercised here. Active-runtime
    // hashlock validation is covered end-to-end at the Logoscore integration
    // layer once Delivery is initialised.
    SwapImpl impl;
    const std::string validHex(64, 'a');
    LOGOS_ASSERT_CONTAINS(impl.subscribeSwap(validHex), R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(impl.subscribeSwap(validHex), "messaging not initialized");
}

LOGOS_TEST(unsubscribe_swap_requires_runtime) {
    SwapImpl impl;
    const std::string validHex(64, 'a');
    LOGOS_ASSERT_CONTAINS(impl.unsubscribeSwap(validHex), R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(impl.unsubscribeSwap(validHex), "messaging not initialized");
}

LOGOS_TEST(publish_swap_accept_requires_runtime) {
    SwapImpl impl;
    LOGOS_ASSERT_CONTAINS(impl.publishSwapAccept("{}"), R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(impl.publishSwapAccept("{}"), "messaging not initialized");
}

LOGOS_TEST(fetch_swap_events_returns_empty_shape_without_runtime) {
    SwapImpl impl;
    const std::string validHex(64, 'b');
    const auto events = impl.fetchSwapEvents(validHex);
    LOGOS_ASSERT_CONTAINS(events, R"("method":"fetchSwapEvents")");
    LOGOS_ASSERT_CONTAINS(events, R"("backend":"delivery_module")");
    LOGOS_ASSERT_CONTAINS(events, R"("events":[])");
}

LOGOS_TEST(run_maker_emits_hashlock_in_eth_lock_detected) {
    SwapImpl impl;
    std::string progressData;
    impl.emitEvent = [&](const std::string& name, const std::string& data) {
        if (name == "maker.progress" && data.find("EthLockDetected") != std::string::npos) {
            progressData = data;
        }
    };
    LOGOS_ASSERT_CONTAINS(impl.runMaker("{}", ""), R"("method":"runMaker")");
    LOGOS_ASSERT_CONTAINS(progressData, R"("step":"EthLockDetected")");
    LOGOS_ASSERT_CONTAINS(progressData, R"("hashlock":")");
}

// ── Onboarding (generate/init/fund) ─────────────────────────────────────────

LOGOS_TEST(generate_eth_key_returns_ffi_json) {
    SwapImpl impl;
    const auto result = impl.generateEthKey();
    LOGOS_ASSERT_CONTAINS(result, R"("private_key":"0x)");
    LOGOS_ASSERT_CONTAINS(result, R"("address":"0x)");
}

LOGOS_TEST(generate_lez_account_returns_ffi_json) {
    SwapImpl impl;
    const auto result = impl.generateLezAccount();
    LOGOS_ASSERT_CONTAINS(result, R"("signing_key":")");
    LOGOS_ASSERT_CONTAINS(result, R"("account_id":")");
}

LOGOS_TEST(lez_ensure_initialized_returns_ffi_json) {
    SwapImpl impl;
    const auto result = impl.lezEnsureInitialized("https://testnet.lez.logos.co", "aa");
    LOGOS_ASSERT_CONTAINS(result, R"("outcome":"Initialized")");
    LOGOS_ASSERT_CONTAINS(result, R"("tx_hash":")");
}

LOGOS_TEST(lez_funding_job_returns_status_and_finished_event) {
    SwapImpl impl;
    std::mutex mutex;
    std::vector<std::string> events;
    impl.emitEvent = [&](const std::string& name, const std::string& data) {
        std::lock_guard<std::mutex> lock(mutex);
        events.push_back(name + ":" + data);
    };

    const auto started = impl.startLezFundingJob("https://testnet.lez.logos.co", "aa", "150");
    LOGOS_ASSERT_CONTAINS(started, R"("ok":true)");
    LOGOS_ASSERT_CONTAINS(started, R"("role":"lez_setup")");
    const auto jobId = extractJobId(started);
    LOGOS_ASSERT_FALSE(jobId.empty());
    LOGOS_ASSERT(waitForContains(impl, jobId, R"("status":"completed")"));

    LOGOS_ASSERT_TRUE(waitForEvent(mutex, events,
                                   "lez_setup.progress:",
                                   std::string{R"("job_id":")"} + jobId + R"(")"));
    LOGOS_ASSERT_TRUE(waitForEvent(mutex, events, "lez_setup.finished:", R"("balance":"150")"));
}

LOGOS_TEST(conflicting_lez_funding_job_is_rejected) {
    SwapImpl impl;
    const auto first = impl.startLezFundingJob("https://testnet.lez.logos.co", "aa", "150");
    const auto jobId = extractJobId(first);
    LOGOS_ASSERT_FALSE(jobId.empty());

    const auto second = impl.startLezFundingJob("https://testnet.lez.logos.co", "aa", "150");
    LOGOS_ASSERT_CONTAINS(second, R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(second, "lez_setup job already running");
    LOGOS_ASSERT(waitForContains(impl, jobId, R"("status":"completed")"));
}

LOGOS_TEST(lez_funding_job_defaults_empty_target_to_one_pinata_claim) {
    SwapImpl impl;
    // An empty target string falls back to kDefaultSetupFundingTargetLez
    // (150 — one pinata claim); this just proves the job still starts and
    // completes rather than rejecting an empty argument.
    const auto started = impl.startLezFundingJob("https://testnet.lez.logos.co", "aa", "");
    const auto jobId = extractJobId(started);
    LOGOS_ASSERT_FALSE(jobId.empty());
    LOGOS_ASSERT(waitForContains(impl, jobId, R"("status":"completed")"));
}

// Note: the rest of the API delegates straight into libswap_ffi.{dylib,so} which
// in turn talks to ETH and LEZ. Real coverage belongs in cargo tests at the
// Rust orchestrator layer (src/), or in logoscore-driven integration tests once
// libswap_ffi is vendored into ./lib/ and a localnet is up. Keep this file
// focused on the C++ wrapper surface.
