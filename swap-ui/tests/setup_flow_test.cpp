// Standalone unit test for swap-ui/src/setup_flow.h — the hidden
// SWAP_UI_LEZ_FAUCET_MODE flag that selects the faucet-less taker Setup
// (issue #166). The load-bearing property is the DEFAULT: anything other
// than an exact "off" must select today's flow, so a typo or an unrelated
// value can never silently change the shipped onboarding.
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

    // --- Default: today's flow. ---
    expect("unset (empty) -> On", parseLezFaucetMode("") == LezFaucetMode::On, true);
    expect("whitespace only -> On", parseLezFaucetMode("  \t ") == LezFaucetMode::On, true);
    expect("\"on\" -> On", parseLezFaucetMode("on") == LezFaucetMode::On, true);
    expect("\"ON\" -> On", parseLezFaucetMode("ON") == LezFaucetMode::On, true);

    // Unrecognised values are NOT a boolean-ish "off": they fall back to
    // today's flow. This is the property that protects the default.
    expect("\"0\" -> On (unrecognised)", parseLezFaucetMode("0") == LezFaucetMode::On, true);
    expect("\"false\" -> On (unrecognised)", parseLezFaucetMode("false") == LezFaucetMode::On, true);
    expect("\"no\" -> On (unrecognised)", parseLezFaucetMode("no") == LezFaucetMode::On, true);
    expect("\"faucetless\" -> On (unrecognised)",
           parseLezFaucetMode("faucetless") == LezFaucetMode::On, true);
    expect("\"of\" -> On (unrecognised)", parseLezFaucetMode("of") == LezFaucetMode::On, true);
    expect("\"off!\" -> On (unrecognised)", parseLezFaucetMode("off!") == LezFaucetMode::On, true);
    expect("\"off off\" -> On (unrecognised)",
           parseLezFaucetMode("off off") == LezFaucetMode::On, true);

    // --- The one opt-in spelling, tolerant of case and surrounding space. ---
    expect("\"off\" -> Off", parseLezFaucetMode("off") == LezFaucetMode::Off, true);
    expect("\"OFF\" -> Off", parseLezFaucetMode("OFF") == LezFaucetMode::Off, true);
    expect("\"Off\" -> Off", parseLezFaucetMode("Off") == LezFaucetMode::Off, true);
    expect("\" off \" -> Off (trimmed)", parseLezFaucetMode(" off ") == LezFaucetMode::Off, true);
    expect("\"off\\n\" -> Off (trimmed)", parseLezFaucetMode("off\n") == LezFaucetMode::Off, true);

    // --- The boolean the UI consumes mirrors the enum exactly. ---
    expect("lezFaucetless(\"\") is false", lezFaucetless(""), false);
    expect("lezFaucetless(\"on\") is false", lezFaucetless("on"), false);
    expect("lezFaucetless(\"garbage\") is false", lezFaucetless("garbage"), false);
    expect("lezFaucetless(\"off\") is true", lezFaucetless("off"), true);

    if (failures != 0) {
        std::fprintf(stderr, "%d failure(s)\n", failures);
        return EXIT_FAILURE;
    }
    std::printf("ALL PASSED\n");
    return EXIT_SUCCESS;
}
