// Where the Setup step's "Get test ETH" button sends its request.
//
// PoC (see README-poc.md): the in-house Sepolia drip faucet lives in
// `eth-faucet/` and is not deployed anywhere yet, so the compiled default
// points at a LOCAL instance — `make faucet-poc-run` on the same machine.
// That makes the button demoable out of the box for a reviewer and, just as
// importantly, makes a build that was never pointed at a real faucet fail
// visibly (nothing listening on 127.0.0.1) rather than silently reach some
// third party.
//
// BEFORE ANY RELEASE the default must become the deployed VPS URL, or the
// button must be hidden. A shipped app whose faucet default is localhost is a
// dead button with a confusing error, which is worse than the external-faucet
// copy it sits beside.
//
// SWAP_UI_ETH_FAUCET_URL overrides it. An explicit empty value ("" or blank)
// DISABLES the button — that is the switch for a build that should show only
// the external faucet links, and the reason "unset" and "set to empty" mean
// different things here.
//
// Deliberately has zero Qt/logos/nix dependencies so it compiles in the
// plain-C++ swap-ui unit harness (same rule as setup_flow.h):
//
//   c++ -std=c++17 -Iswap-ui/src swap-ui/tests/eth_faucet_config_test.cpp && ./a.out

#pragma once

#include <string>

namespace swap_ui {

// Environment variable name, single-sourced so the plugin and this file's
// docs cannot drift.
inline constexpr const char* kEthFaucetUrlEnv = "SWAP_UI_ETH_FAUCET_URL";

// The compiled default — a locally-run PoC service. See the header comment.
inline constexpr const char* kDefaultEthFaucetUrl = "http://127.0.0.1:8787";

// Trim ASCII whitespace and any trailing slashes, so "http://host:8787/" and
// "http://host:8787" name the same faucet. The client appends "/challenge"
// and "/drip", so a trailing slash would otherwise produce "//challenge" —
// which most servers tolerate and some proxies do not.
inline std::string normalizeFaucetUrl(const std::string& raw)
{
    const auto begin = raw.find_first_not_of(" \t\r\n");
    if (begin == std::string::npos) {
        return {};
    }
    const auto end = raw.find_last_not_of(" \t\r\n");
    std::string value = raw.substr(begin, end - begin + 1);
    while (!value.empty() && value.back() == '/') {
        value.pop_back();
    }
    return value;
}

// Resolve the faucet URL from an environment value.
//
// `isSet` distinguishes "the variable is absent" (use the compiled default)
// from "the variable is present and empty" (the operator turned the button
// off). Collapsing those two would make it impossible to disable the button
// without also knowing an unreachable URL to point it at.
inline std::string ethFaucetUrl(const std::string& raw, bool isSet)
{
    if (!isSet) {
        return kDefaultEthFaucetUrl;
    }
    return normalizeFaucetUrl(raw);
}

// Only http(s) URLs are usable — the client speaks JSON over HTTP. Anything
// else (a stray "localhost:8787", a ws:// paste) is treated as "no faucet"
// rather than as a URL that will fail at request time with a transport error
// nobody can read.
inline bool isUsableFaucetUrl(const std::string& url)
{
    return url.rfind("http://", 0) == 0 || url.rfind("https://", 0) == 0;
}

// How long the app waits for the whole faucet round trip.
//
// The generated `faucetRequestEthAsync` wrapper defaults to a 20s Timeout, and
// an expiry arrives as an EMPTY QString rather than an error — the shape of
// issue #171, where an activation that succeeded on-chain read as a blank
// failure. One request is a proof-of-work solve (seconds to low minutes) plus a
// Sepolia inclusion wait, so 20s would expire on essentially every honest
// request. Five minutes covers both with room to spare; past it the app says so
// plainly rather than showing nothing.
inline constexpr int kFaucetRequestTimeoutMs = 300 * 1000;

// Phase names published on `setupStep` during a request, so SetupView's
// per-phase elapsed ticker has something to restart on and the card can say
// which kind of waiting is going on. Namespaced with a "Faucet" prefix because
// setupStep is shared with the LEZ funding job's own step vocabulary.
inline constexpr const char* kStepFaucetRequesting = "FaucetRequesting";
inline constexpr const char* kStepFaucetDripped = "FaucetDripped";

} // namespace swap_ui
