#ifndef SWAP_UI_BALANCE_READ_GATE_H
#define SWAP_UI_BALANCE_READ_GATE_H

#include <string>

// Pure, dependency-free readiness check for a BALANCE READ, shared between
// the plugin (swap_ui_plugin.cpp, fetchBalances / applyBalancesResult) and its
// unit test (tests/balance_read_gate_test.cpp). Deliberately excludes Qt so
// the test compiles standalone (see `make swap-ui-unit`).
//
// Reading a balance needs an RPC endpoint and an account per chain — nothing
// else. fetchBalances() used to gate on the FULL swap-form validation
// (amounts, recipient, timelocks, poll interval, program ID...), so on a fresh
// install the automatic refresh fired while the user was still on the Setup
// tab, failed on swap fields they had not reached yet, and published a
// global red "Fix validation errors before fetching balances" banner over a
// Setup screen that was working correctly. This gate validates only what a
// balance read actually uses; the full form check stays where it belongs, on
// the swap actions (SwapUiPlugin::validateConfig / validateConfigForAction).
//
// Per chain, not all-or-nothing: after Setup step 1 (Ethereum key) the ETH
// balance is readable while the LEZ account does not exist yet. A side that
// is not set up is simply not read (and its "not set up yet" result is not
// an error worth a banner — see `sideNotSetUp`). Mirrors
// `parse_balance_config` in swap-ffi/src/lib.rs, which applies the same rule
// on the Rust side of the call.
namespace swap_ui {

struct BalanceReadGate {
    bool ethReady = false;
    bool lezReady = false;
    // At least one chain can be read.
    bool anyReady() const { return ethReady || lezReady; }
};

namespace detail {

inline std::string trimAscii(const std::string& value)
{
    size_t begin = 0;
    size_t end = value.size();
    auto isSpace = [](char c) {
        return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f' || c == '\v';
    };
    while (begin < end && isSpace(value[begin])) ++begin;
    while (end > begin && isSpace(value[end - 1])) --end;
    return value.substr(begin, end - begin);
}

// Same rule as SwapUiPlugin::isHexBytes: optional 0x prefix, exactly `bytes`
// bytes of hex.
inline bool isHexBytes(const std::string& value, size_t bytes)
{
    std::string clean = trimAscii(value);
    if (clean.size() >= 2 && clean[0] == '0' && (clean[1] == 'x' || clean[1] == 'X')) {
        clean = clean.substr(2);
    }
    if (clean.size() != bytes * 2) return false;
    for (char c : clean) {
        const bool hex = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
        if (!hex) return false;
    }
    return true;
}

inline bool present(const std::string& value)
{
    return !trimAscii(value).empty();
}

} // namespace detail

// ETH side: an RPC URL plus a well-formed 32-byte key (the address is derived
// from it). The HTLC contract address is included because the ETH client
// performs its version handshake against it on connect; it is a pinned
// default the user never types, so it costs a fresh install nothing.
// LEZ side: a sequencer URL plus EITHER a well-formed raw signing key OR a
// wallet home + account ID pair.
inline BalanceReadGate balanceReadGate(const std::string& ethRpcUrl,
                                       const std::string& ethPrivateKey,
                                       const std::string& ethHtlcAddress,
                                       const std::string& lezSequencerUrl,
                                       const std::string& lezSigningKey,
                                       const std::string& lezWalletHome,
                                       const std::string& lezAccountId)
{
    using namespace detail;
    BalanceReadGate gate;
    gate.ethReady = present(ethRpcUrl)
        && isHexBytes(ethPrivateKey, 32)
        && isHexBytes(ethHtlcAddress, 20);
    const bool hasRawKey = isHexBytes(lezSigningKey, 32);
    const bool hasWallet = present(lezWalletHome) && present(lezAccountId);
    gate.lezReady = present(lezSequencerUrl) && (hasRawKey || hasWallet);
    return gate;
}

// True when a per-side balance error only says that side is not set up yet
// (swap-ffi reports "Ethereum key not set up yet" / "LEZ account not set up
// yet" for a side the gate skipped). That is expected mid-Setup, not a
// failure the user should be shown.
inline bool sideNotSetUp(const std::string& sideError)
{
    return sideError.find("not set up yet") != std::string::npos;
}

} // namespace swap_ui

#endif // SWAP_UI_BALANCE_READ_GATE_H
