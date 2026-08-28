// Standalone unit test for swap-ui/src/setup_flow.h — the developer override
// SWAP_UI_LEZ_FAUCET_MODE, which restores the legacy faucet Setup (issue
// #166). The load-bearing property is the DEFAULT: the faucet-less flow now
// ships, so anything other than an exact "on" must select it, and a typo or
// an unrelated value can never silently hand a user the legacy onboarding.
//
// Note the fallback follows the default, and the default was inverted — so
// these expectations are the mirror image of the ones this file first
// carried. That is the change, not a regression: see setup_flow.h.
//
// Deliberately has zero Qt/logos/nix dependencies so it can be compiled and
// run directly:
//
//   c++ -std=c++17 -I../src -o /tmp/setup_flow_test setup_flow_test.cpp \
//     && /tmp/setup_flow_test
//
// Exits 0 and prints "ALL PASSED" on success; exits non-zero with a message
// per failing case otherwise.

#include "../src/setup_flow.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

namespace {

int failures = 0;

void expect(const char* label, bool actual, bool expected)
{
    if (actual != expected) {
        std::fprintf(stderr, "FAIL %s: got %s, want %s\n", label,
                     actual ? "true" : "false", expected ? "true" : "false");
        ++failures;
    } else {
        std::printf("ok   %s\n", label);
    }
}

} // namespace

int main()
{
    using swap_ui::LezFaucetMode;
    using swap_ui::lezFaucetless;
    using swap_ui::parseLezFaucetMode;

    // The flag name is what the plugin reads and what the docs name.
    expect("env var name is SWAP_UI_LEZ_FAUCET_MODE",
           std::strcmp(swap_ui::kLezFaucetModeEnv, "SWAP_UI_LEZ_FAUCET_MODE") == 0, true);

    // --- Default: the shipped faucet-less flow. ---
    expect("unset (empty) -> Off", parseLezFaucetMode("") == LezFaucetMode::Off, true);
    expect("whitespace only -> Off", parseLezFaucetMode("  \t ") == LezFaucetMode::Off, true);
    expect("\"off\" -> Off", parseLezFaucetMode("off") == LezFaucetMode::Off, true);
    expect("\"OFF\" -> Off", parseLezFaucetMode("OFF") == LezFaucetMode::Off, true);

    // Unrecognised values are NOT a boolean-ish "on": they fall back to the
    // shipped default. This is the property that protects the default, and it
    // now protects the faucet-LESS flow.
    expect("\"1\" -> Off (unrecognised)", parseLezFaucetMode("1") == LezFaucetMode::Off, true);
    expect("\"true\" -> Off (unrecognised)", parseLezFaucetMode("true") == LezFaucetMode::Off, true);
    expect("\"yes\" -> Off (unrecognised)", parseLezFaucetMode("yes") == LezFaucetMode::Off, true);
    expect("\"faucet\" -> Off (unrecognised)",
           parseLezFaucetMode("faucet") == LezFaucetMode::Off, true);
    expect("\"o\" -> Off (unrecognised)", parseLezFaucetMode("o") == LezFaucetMode::Off, true);
    expect("\"on!\" -> Off (unrecognised)", parseLezFaucetMode("on!") == LezFaucetMode::Off, true);
    expect("\"on on\" -> Off (unrecognised)",
           parseLezFaucetMode("on on") == LezFaucetMode::Off, true);
    // "off" is the value the pre-default-flip tooling and demo scripts pass;
    // it still means faucet-less even with stray whitespace, so nothing that
    // set it breaks.
    expect("\" off \" -> Off (trimmed)", parseLezFaucetMode(" off ") == LezFaucetMode::Off, true);
    expect("\"off\\n\" -> Off (trimmed)", parseLezFaucetMode("off\n") == LezFaucetMode::Off, true);

    // --- The one opt-in spelling for the legacy flow, tolerant of case and
    // surrounding space. ---
    expect("\"on\" -> On", parseLezFaucetMode("on") == LezFaucetMode::On, true);
    expect("\"ON\" -> On", parseLezFaucetMode("ON") == LezFaucetMode::On, true);
    expect("\"On\" -> On", parseLezFaucetMode("On") == LezFaucetMode::On, true);
    expect("\" on \" -> On (trimmed)", parseLezFaucetMode(" on ") == LezFaucetMode::On, true);
    expect("\"on\\n\" -> On (trimmed)", parseLezFaucetMode("on\n") == LezFaucetMode::On, true);

    // --- The boolean the UI consumes mirrors the enum exactly. ---
    expect("lezFaucetless(\"\") is true (the default)", lezFaucetless(""), true);
    expect("lezFaucetless(\"off\") is true", lezFaucetless("off"), true);
    expect("lezFaucetless(\"garbage\") is true", lezFaucetless("garbage"), true);
    expect("lezFaucetless(\"on\") is false", lezFaucetless("on"), false);

    if (failures != 0) {
        std::fprintf(stderr, "%d failure(s)\n", failures);
        return EXIT_FAILURE;
    }
    std::printf("ALL PASSED\n");
    return EXIT_SUCCESS;
}
