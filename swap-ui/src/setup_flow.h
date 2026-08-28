// Guided-Setup flow selection: which LEZ onboarding path the Setup tab runs.
//
// Why this exists (issue #166, faucet-less taker Setup): a taker never needs
// a LEZ balance to complete a swap. LEZ v0.2.2 charges no transaction fees,
// so the only on-chain prerequisite is that the taker's account is
// INITIALIZED (owned by the authenticated_transfer program) — a free, signed,
// one-transaction step that already exists as SwapImpl::lezEnsureInitialized.
// The 150-LEZ pinata claim in today's step 3 is pure surplus for a taker.
//
// The faucet-less path is exploratory product work behind a hidden developer
// flag, `SWAP_UI_LEZ_FAUCET_MODE` (read in swap_ui_plugin.cpp next to the
// other SWAP_UI_* overrides). This header holds the pure decision so the
// plain-C++ unit harness (`make swap-ui-unit`) can pin it down:
//
//   * `off`            -> faucet-less Setup (generate key -> initialize ->
//                          get Sepolia ETH -> trade).
//   * `on`, unset, or ANY unrecognised value -> today's flow, byte-for-byte.
//
// The default MUST stay today's flow: the value is a fail-safe, not a
// boolean, so a typo ("Off " is fine — trimmed and case-folded — but "0",
// "false", "no" are not) never silently changes the shipped onboarding.
//
// Deliberately has zero Qt/logos/nix dependencies so it compiles in the
// plain-C++ swap-ui unit harness:
//
//   c++ -std=c++17 -Iswap-ui/src swap-ui/tests/setup_flow_test.cpp && ./a.out

#pragma once

#include <algorithm>
#include <cctype>
#include <string>

namespace swap_ui {

// Environment variable name, single-sourced so the plugin and the docs
// comment cannot drift.
inline constexpr const char* kLezFaucetModeEnv = "SWAP_UI_LEZ_FAUCET_MODE";

enum class LezFaucetMode {
    // Today's Setup: step 3 initializes AND claims 150 LEZ from pinata.
    On,
    // Faucet-less Setup: step 3 only initializes (free, one transaction).
    Off,
};

// Map the raw environment value to a mode. Only an exact (trimmed,
// case-insensitive) "off" selects the faucet-less path; everything else —
// including unset/empty — is today's flow.
inline LezFaucetMode parseLezFaucetMode(const std::string& raw)
{
    const auto begin = raw.find_first_not_of(" \t\r\n");
    if (begin == std::string::npos) {
        return LezFaucetMode::On;
    }
    const auto end = raw.find_last_not_of(" \t\r\n");
    std::string value = raw.substr(begin, end - begin + 1);
    std::transform(value.begin(), value.end(), value.begin(),
                   [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    return value == "off" ? LezFaucetMode::Off : LezFaucetMode::On;
}

// Convenience for the one question the UI asks.
inline bool lezFaucetless(const std::string& raw)
{
    return parseLezFaucetMode(raw) == LezFaucetMode::Off;
}

} // namespace swap_ui
