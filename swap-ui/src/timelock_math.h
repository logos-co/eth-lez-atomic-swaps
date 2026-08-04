#ifndef SWAP_UI_TIMELOCK_MATH_H
#define SWAP_UI_TIMELOCK_MATH_H

#include <cstdint>

// Pure, dependency-free timelock arithmetic shared between the plugin
// (swap_ui_plugin.cpp, applyOfferObject) and its unit test
// (tests/timelock_math_test.cpp). Deliberately excludes Qt/logos headers so
// the test can compile standalone (no nix build / module toolchain needed)
// and so the exact conversion applyOfferObject relies on is exercised
// directly, not reimplemented in the test.
namespace swap_ui {

// Convert an absolute unix-seconds expiry into "minutes from now", rounded
// UP (ceil) so a taker's timelock-minutes config never falls short of what
// the offer actually advertised, and clamped to a minimum of 1 so an
// already-tight or already-expired offer still produces a valid positive
// config value instead of 0 or negative (which would fail the plugin's own
// "must be positive minutes" validation).
inline int64_t minutesUntilExpiry(int64_t expirySecs, int64_t nowSecs)
{
    if (expirySecs <= nowSecs) {
        return 1;
    }
    const int64_t minutes = (expirySecs - nowSecs + 59) / 60; // ceil div by 60
    return minutes > 0 ? minutes : 1;
}

} // namespace swap_ui

#endif // SWAP_UI_TIMELOCK_MATH_H
