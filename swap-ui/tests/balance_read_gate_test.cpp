// Standalone unit test for swap-ui/src/balance_read_gate.h — the gate
// SwapUiPlugin::fetchBalances() uses instead of the full swap-form
// validation. The regression it pins: a config holding only an RPC URL and a
// key must PASS the balance path, while the swap path (validateConfig, which
// still demands amounts/recipient/timelocks) keeps rejecting it. The swap-path
// half lives in the plugin (Qt) and swap-ffi (`balance_only_config_still_fails
// _the_swap_path` in swap-ffi/src/lib.rs); this test covers the gate itself.
//
//   c++ -std=c++17 -I../src -o /tmp/balance_read_gate_test balance_read_gate_test.cpp \
//     && /tmp/balance_read_gate_test

#include "../src/balance_read_gate.h"

#include <cstdio>
#include <string>

namespace {

int failures = 0;

void expect(const char* label, bool cond)
{
    if (!cond) {
        std::fprintf(stderr, "FAIL %s\n", label);
        ++failures;
    } else {
        std::printf("ok   %s\n", label);
    }
}

const std::string kRpc = "wss://ethereum-sepolia-rpc.publicnode.com";
const std::string kKey = "0x1111111111111111111111111111111111111111111111111111111111111111";
const std::string kHtlc = "0x351B0EA07739FA9F6769213927D7836a790A5FAF";
const std::string kSeq = "http://sequencer.example:3040";
const std::string kLezKey = "2222222222222222222222222222222222222222222222222222222222222222";

} // namespace

int main()
{
    using swap_ui::balanceReadGate;
    using swap_ui::sideNotSetUp;

    // The on-camera case: Setup step 1 done (ETH key), nothing else. The form
    // still has NO amount, recipient, timelock, program ID or LEZ account.
    {
        auto g = balanceReadGate(kRpc, kKey, kHtlc, kSeq, "", "", "");
        expect("rpc url + key: ETH side readable", g.ethReady);
        expect("rpc url + key: LEZ side not yet", !g.lezReady);
        expect("rpc url + key: balance path passes", g.anyReady());
    }

    // Fresh install before step 1: defaults only, no key at all.
    {
        auto g = balanceReadGate(kRpc, "", kHtlc, kSeq, "", "", "");
        expect("no key anywhere: nothing readable", !g.anyReady());
    }

    // LEZ-only setup (raw key).
    {
        auto g = balanceReadGate("", "", "", kSeq, kLezKey, "", "");
        expect("lez raw key: LEZ readable", g.lezReady);
        expect("lez raw key: ETH not", !g.ethReady);
    }

    // LEZ via wallet home + account ID (no raw key).
    {
        auto g = balanceReadGate("", "", "", kSeq, "", "/home/x/.wallet", "7YXq9G");
        expect("lez wallet auth: LEZ readable", g.lezReady);
        auto half = balanceReadGate("", "", "", kSeq, "", "/home/x/.wallet", "");
        expect("lez wallet home without account id: not readable", !half.lezReady);
    }

    // Endpoint missing blocks that side even with an account.
    {
        expect("eth: empty rpc url blocks", !balanceReadGate("", kKey, kHtlc, "", "", "", "").ethReady);
        expect("eth: whitespace rpc url blocks", !balanceReadGate("   ", kKey, kHtlc, "", "", "", "").ethReady);
        expect("lez: empty sequencer blocks", !balanceReadGate("", "", "", "", kLezKey, "", "").lezReady);
    }

    // Malformed key is not "an address" — it must not pass.
    {
        expect("eth: short key rejected", !balanceReadGate(kRpc, "0xabc", kHtlc, "", "", "", "").ethReady);
        expect("eth: non-hex key rejected",
               !balanceReadGate(kRpc, "0xzz11111111111111111111111111111111111111111111111111111111111111", kHtlc, "", "", "", "").ethReady);
        expect("eth: key without 0x accepted", balanceReadGate(kRpc, kKey.substr(2), kHtlc, "", "", "", "").ethReady);
        expect("eth: malformed htlc address rejected", !balanceReadGate(kRpc, kKey, "0x1234", "", "", "", "").ethReady);
        expect("lez: short signing key rejected", !balanceReadGate("", "", "", kSeq, "22", "", "").lezReady);
    }

    // "not set up yet" side results are expected mid-Setup, not banners.
    {
        expect("ffi 'Ethereum key not set up yet' is quiet", sideNotSetUp("Ethereum key not set up yet"));
        expect("ffi 'LEZ account not set up yet' is quiet", sideNotSetUp("LEZ account not set up yet"));
        expect("a real RPC failure is not quiet", !sideNotSetUp("WebSocket connect failed: timeout"));
        expect("empty error is not 'not set up'", !sideNotSetUp(""));
    }

    if (failures) {
        std::fprintf(stderr, "%d FAILED\n", failures);
        return 1;
    }
    std::printf("ALL PASSED\n");
    return 0;
}
