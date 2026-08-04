// Standalone unit test for swap-ui/src/timelock_math.h — the absolute-unix-
// expiry -> minutes-from-now conversion that SwapUiPlugin::applyOfferObject
// uses to adopt an accepted offer's own timelocks (see PR
// fix/taker-adopts-offer-timelocks). Deliberately has zero Qt/logos/nix
// dependencies so it can be compiled and run directly:
//
//   c++ -std=c++17 -I../src -o /tmp/timelock_math_test timelock_math_test.cpp \
//     && /tmp/timelock_math_test
//
// Exits 0 and prints "ALL PASSED" on success; aborts with a message on the
// first failing case otherwise.

#include "../src/timelock_math.h"

#include <cstdio>
#include <cstdlib>

namespace {

int failures = 0;

void expect_eq(const char* label, int64_t actual, int64_t expected)
{
    if (actual != expected) {
        std::fprintf(stderr, "FAIL %s: got %lld, want %lld\n",
                     label, static_cast<long long>(actual),
                     static_cast<long long>(expected));
        ++failures;
    } else {
        std::printf("ok   %s\n", label);
    }
}

} // namespace

int main()
{
    using swap_ui::minutesUntilExpiry;
    const int64_t now = 1'800'000'000; // arbitrary fixed epoch for determinism

    // The PR's motivating scenario: a maker-loop offer advertising LEZ 20 /
    // ETH 40 minutes. A taker's applyOfferObject must reproduce those exact
    // minute values from the offer's absolute expiries.
    expect_eq("offer lez=20min -> 20", minutesUntilExpiry(now + 20 * 60, now), 20);
    expect_eq("offer eth=40min -> 40", minutesUntilExpiry(now + 40 * 60, now), 40);

    // Exact multiple of 60 seconds: no rounding should occur.
    expect_eq("exact 5 minutes", minutesUntilExpiry(now + 300, now), 5);

    // Ceil rounding: 61s from now is more than 1 minute away, must round UP
    // to 2 — a taker must never adopt a shorter timelock than the offer
    // actually has left (rounding down could reproduce exactly the "boundary
    // exact" bug this repo has already hit once with the 10/5/5 defaults).
    expect_eq("61s ceils to 2min", minutesUntilExpiry(now + 61, now), 2);
    expect_eq("1s ceils to 1min", minutesUntilExpiry(now + 1, now), 1);
    expect_eq("59s ceils to 1min", minutesUntilExpiry(now + 59, now), 1);

    // Clamp to >=1: an expiry at or before "now" (already expired, or a
    // clock-skew edge case) must still produce a valid positive minutes
    // value, never 0 or negative (0 fails the plugin's own "must be positive
    // minutes" validation and would silently block the taker from ever
    // adopting the offer).
    expect_eq("expiry == now clamps to 1", minutesUntilExpiry(now, now), 1);
    expect_eq("expiry 1s in the past clamps to 1", minutesUntilExpiry(now - 1, now), 1);
    expect_eq("expiry far in the past clamps to 1", minutesUntilExpiry(now - 100000, now), 1);

    // Large offer window sanity check.
    expect_eq("offer 3 hours -> 180min", minutesUntilExpiry(now + 3 * 3600, now), 180);

    if (failures > 0) {
        std::fprintf(stderr, "%d assertion(s) FAILED\n", failures);
        return 1;
    }
    std::printf("ALL PASSED\n");
    return 0;
}
