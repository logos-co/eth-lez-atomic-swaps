#include <logos_test.h>
#include "../src/swap_impl.h"

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
    std::string eventName;
    std::string eventData;
    impl.emitEvent = [&](const std::string& name, const std::string& data) {
        eventName = name;
        eventData = data;
    };

    LOGOS_ASSERT_CONTAINS(impl.runMaker("{}", ""), R"("method":"runMaker")");
    LOGOS_ASSERT_EQ(eventName, std::string("maker.progress"));
    LOGOS_ASSERT_CONTAINS(eventData, "maker-started");
}

// Note: the rest of the API delegates straight into libswap_ffi.{dylib,so} which
// in turn talks to ETH and LEZ. Real coverage belongs in cargo tests at the
// Rust orchestrator layer (src/), or in logoscore-driven integration tests once
// libswap_ffi is vendored into ./lib/ and a localnet is up. Keep this file
// focused on the C++ wrapper surface.
