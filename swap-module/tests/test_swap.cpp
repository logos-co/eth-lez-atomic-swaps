#include <logos_test.h>
#include "../src/swap_impl.h"

#include <QCoreApplication>

#include <chrono>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

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

// Note: the rest of the API delegates straight into libswap_ffi.{dylib,so} which
// in turn talks to ETH and LEZ. Real coverage belongs in cargo tests at the
// Rust orchestrator layer (src/), or in logoscore-driven integration tests once
// libswap_ffi is vendored into ./lib/ and a localnet is up. Keep this file
// focused on the C++ wrapper surface.
