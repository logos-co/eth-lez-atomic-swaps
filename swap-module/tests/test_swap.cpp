#include <logos_test.h>
#include "../src/swap_impl.h"

#include <QCoreApplication>
#include <QString>

#include <chrono>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

std::string swapDeliveryEthAmountToWei(const std::string& ethAmount);
int swapDeliveryParsePeerCount(const std::string& raw);
QString swapDeliveryConfigJson(const std::string& configJson);

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

// getNodeInfo returns node-info items as strings of JSON-serializable data;
// the peer-count parser must accept every plausible encoding and refuse to
// invent a count from garbage (returning -1 = unknown, never a fake 0).
LOGOS_TEST(delivery_peer_count_parses_bare_and_wrapped_numbers) {
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("3"), 3);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount(" 12 \n"), 12);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("\"7\""), 7);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("0"), 0);
}

LOGOS_TEST(delivery_peer_count_counts_json_array_elements) {
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("[]"), 0);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("[ ]"), 0);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount(R"(["16Uiu2HAm","16Uiu2HAn"])"), 2);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount(R"([{"peerId":"a"},{"peerId":"b"},{"peerId":"c"}])"), 3);
    // Nested structures and escaped/comma-bearing strings must not skew the
    // top-level element count.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount(R"([["a","b"],["c"]])"), 2);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount(R"(["with,comma","with\"quote"])"), 2);
}

LOGOS_TEST(delivery_peer_count_rejects_garbage_as_unknown) {
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount(""), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("   "), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("waku"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("-1"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("3 peers"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("[1,2"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount(R"({"peerCount":3})"), -1);
}

LOGOS_TEST(delivery_peer_count_rejects_malformed_arrays_and_out_of_range) {
    // Trailing / empty-segment commas must not be counted as elements.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("[1,]"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("[,1]"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("[1,,2]"), -1);
    // Mismatched delimiters are malformed, not countable.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("[1}"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount(R"([{"a":1]])"), -1);
    // int-range boundary: INT_MAX parses, anything above reads as unknown.
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("2147483647"), 2147483647);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("2147483648"), -1);
    LOGOS_ASSERT_EQ(swapDeliveryParsePeerCount("99999999999999999999"), -1);
}

LOGOS_TEST(delivery_messaging_requires_runtime_before_init_or_publish) {
    SwapImpl impl;
    LOGOS_ASSERT_CONTAINS(impl.messagingInit("{}"), R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(impl.messagingInit("{}"), "delivery_module runtime");
    LOGOS_ASSERT_CONTAINS(impl.publishOffer("{}"), R"("ok":false)");
    LOGOS_ASSERT_CONTAINS(impl.publishOffer("{}"), "messaging not initialized");
}

LOGOS_TEST(delivery_eth_amount_decimal_normalizes_to_wei) {
    LOGOS_ASSERT_EQ(swapDeliveryEthAmountToWei("0.00000000000000001"), std::string("10"));
}

// The logos.dev fleet migrated Waku cluster 2 -> 3. deliveryConfig must force
// cluster 3 over the still-cluster-2 preset with the flat `clusterId` key ONLY.
// createNode REJECTS unrecognised keys — delivery_module v0.1.1 fails createNode
// with "Unrecognized configuration option(s) found: [numShardsInCluster,
// messagingOverrides]" -> "Failed to create Delivery context" — so those keys
// must never be emitted; the preset already runs 8 autoshards. See PR #117 and
// the delivery-context-creation fix.
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

LOGOS_TEST(fetch_offers_preserves_empty_offers_shape_without_runtime) {
    SwapImpl impl;
    const auto offers = impl.fetchOffers();
    LOGOS_ASSERT_CONTAINS(offers, R"("offers":[])");
    LOGOS_ASSERT_CONTAINS(offers, R"("backend":"delivery_module")");
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
