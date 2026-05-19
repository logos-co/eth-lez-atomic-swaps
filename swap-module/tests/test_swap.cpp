#include <logos_test.h>
#include "../src/swap_impl.h"

#include <QCoreApplication>

#include <chrono>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

std::string swapDeliveryEthAmountToWei(const std::string& ethAmount);

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

// Note: the rest of the API delegates straight into libswap_ffi.{dylib,so} which
// in turn talks to ETH and LEZ. Real coverage belongs in cargo tests at the
// Rust orchestrator layer (src/), or in logoscore-driven integration tests once
// libswap_ffi is vendored into ./lib/ and a localnet is up. Keep this file
// focused on the C++ wrapper surface.
