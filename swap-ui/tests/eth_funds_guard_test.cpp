// Standalone unit test for swap-ui/src/eth_funds_guard.h — the pre-flight
// "can this wallet afford this offer?" check and the insufficient-funds
// error detector (see fix/insufficient-eth-guard: a 0-ETH taker could start
// a swap that was certain to die on "insufficient funds for gas", and the
// raw JSON-RPC text was shown as the headline).
//
// Deliberately has zero Qt/logos/nix dependencies so it can be compiled and
// run directly:
//
//   c++ -std=c++17 -I../src -o /tmp/eth_funds_guard_test eth_funds_guard_test.cpp \
//     && /tmp/eth_funds_guard_test
//
// Exits 0 and prints "ALL PASSED" on success; exits non-zero with a message
// per failing case otherwise.

#include "../src/eth_funds_guard.h"

#include <cstdio>
#include <cstdlib>
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
    using swap_ui::insufficientEthForOffer;
    using swap_ui::isInsufficientEthError;

    // The live bug: balance 0, offer 0.00001 ETH (1e13 wei). Must block.
    expect("zero balance blocks",
           insufficientEthForOffer("0", "10000000000000"), true);

    // Unknown balance (never fetched / fetch failed) must NOT block — the
    // guard stops swaps that are certain to fail, not flaky balance reads.
    expect("empty (unknown) balance never blocks",
           insufficientEthForOffer("", "10000000000000"), false);
    expect("non-numeric balance never blocks",
           insufficientEthForOffer("Fetching balances...", "10000000000000"),
           false);

    // Unparseable offer amount: nothing to judge, never block.
    expect("non-numeric offer never blocks",
           insufficientEthForOffer("10000000000000", ""), false);

    // A balance exactly equal to the offer still lacks gas money.
    expect("offer amount alone is not enough (gas headroom)",
           insufficientEthForOffer("10000000000000", "10000000000000"), true);

    // Exactly offer + headroom is enough; one wei short of it is not.
    expect("offer + headroom exactly is enough",
           insufficientEthForOffer("510000000000000", "10000000000000",
                                   "500000000000000"),
           false);
    expect("one wei below offer + headroom blocks",
           insufficientEthForOffer("509999999999999", "10000000000000",
                                   "500000000000000"),
           true);

    // A comfortably funded wallet passes.
    expect("1 ETH covers a 0.00001 ETH offer",
           insufficientEthForOffer("1000000000000000000", "10000000000000"),
           false);

    // Wei values overflow int64 above ~9.2 ETH; the decimal-string math must
    // stay exact there (100 ETH balance vs 100 ETH offer + headroom).
    expect("beyond-int64 balance short of beyond-int64 offer blocks",
           insufficientEthForOffer("100000000000000000000",
                                   "100000000000000000000"),
           true);
    expect("beyond-int64 balance covering beyond-int64 offer passes",
           insufficientEthForOffer("100000500000000000000",
                                   "100000000000000000000"),
           false);

    // Leading zeros must not change magnitude comparison.
    expect("leading zeros compare by value",
           insufficientEthForOffer("0510000000000000", "10000000000000",
                                   "500000000000000"),
           false);

    // --- Error text detection --------------------------------------------
    expect("live -32000 RPC text is detected",
           isInsufficientEthError(
               "Ethereum RPC error: server returned an error response: "
               "error code -32000: failed with 16777216 gas: insufficient "
               "funds for gas * price + value: address 0x8019...e75B have 0 "
               "want 10000000000000"),
           true);
    expect("detection is case-insensitive",
           isInsufficientEthError("INSUFFICIENT FUNDS for gas"), true);
    expect("unrelated errors pass through",
           isInsufficientEthError("Ethereum RPC error: connection refused"),
           false);
    expect("empty error is not a funds error",
           isInsufficientEthError(""), false);

    if (failures != 0) {
        std::fprintf(stderr, "%d failure(s)\n", failures);
        return EXIT_FAILURE;
    }
    std::printf("ALL PASSED\n");
    return EXIT_SUCCESS;
}
