// Guided-Setup flow selection: which LEZ onboarding path the Setup tab runs.
//
// Why this exists (issue #166): a taker never needs a LEZ balance to complete
// a swap. LEZ v0.2.2 charges no transaction fees, so the only on-chain
// prerequisite is that the taker's account is INITIALIZED (owned by the
// authenticated_transfer program) — a free, signed, one-transaction step that
// already exists as SwapImpl::lezEnsureInitialized. Handing a new user 150 LEZ
// from a faucet before they have traded for any is worse than surplus: it
// teaches the wrong first move. A new user should acquire LEZ by trading for
// it, which is the whole argument in issue #166.
//
// So the faucet-less path is now THE onboarding, and this flag is a developer
// override for tooling and recorded demo scripts that still expect the old
// screens:
//
//   * unset, empty, "off", or ANY unrecognised value -> faucet-less Setup
//     (generate key -> activate account -> get Sepolia ETH -> trade). The
//     shipped default.
//   * "on" -> the legacy faucet flow, where step 3 initializes AND claims
//     150 LEZ from pinata, byte-for-byte as 0.4.5 shipped it.
//
// The unrecognised-value case deliberately resolves to the shipped default
// rather than to the legacy flow: the value is a fail-safe, not a boolean, so
// a typo ("On " is fine — trimmed and case-folded — but "1", "true", "yes"
// are not) never silently hands a user an onboarding path we did not choose
// to ship. Note this is the same rule as before the default was inverted, and
// therefore the opposite answer: the safe fallback follows the default.
//
// The env var is NOT how a user picks the faucet. The faucet itself stays
// reachable in the app on the faucet-less path, as its own named disclosure on
// the Setup page (SetupView.qml's `otherWaysToGetLez`), so someone who wants
// test LEZ without trading — a seller stocking inventory, mainly — can get it
// without an environment variable.
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
    // Legacy Setup: step 3 initializes AND claims 150 LEZ from pinata.
    // Developer override only, selected by an exact "on".
    On,
    // Faucet-less Setup: step 3 only initializes (free, one transaction).
    // The shipped default.
    Off,
};

// Map the raw environment value to a mode. Only an exact (trimmed,
// case-insensitive) "on" selects the legacy faucet flow; everything else —
// including unset/empty — is the shipped faucet-less path.
inline LezFaucetMode parseLezFaucetMode(const std::string& raw)
{
    const auto begin = raw.find_first_not_of(" \t\r\n");
    if (begin == std::string::npos) {
        return LezFaucetMode::Off;
    }
    const auto end = raw.find_last_not_of(" \t\r\n");
    std::string value = raw.substr(begin, end - begin + 1);
    std::transform(value.begin(), value.end(), value.begin(),
                   [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    return value == "on" ? LezFaucetMode::On : LezFaucetMode::Off;
}

// Convenience for the one question the UI asks.
inline bool lezFaucetless(const std::string& raw)
{
    return parseLezFaucetMode(raw) == LezFaucetMode::Off;
}

} // namespace swap_ui
