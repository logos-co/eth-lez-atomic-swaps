// Standalone unit test for swap-ui/src/eth_faucet_config.h — resolving the
// in-house drip faucet's URL for the Setup step's "Get test ETH" button.
//
// The load-bearing property is that "unset" and "set to empty" mean DIFFERENT
// things: absent gives the compiled default, present-and-empty disables the
// button. Collapsing them would leave no way to ship a build with the button
// off short of pointing it at a URL known to fail.
//
// Deliberately has zero Qt/logos/nix dependencies so it can be compiled and
// run directly:
//
//   c++ -std=c++17 -I../src -o /tmp/eth_faucet_config_test \
//     eth_faucet_config_test.cpp && /tmp/eth_faucet_config_test
//
// Exits 0 and prints "ALL PASSED" on success; exits non-zero with a message
// per failing case otherwise.

#include "../src/eth_faucet_config.h"

#include <cstdio>
#include <cstdlib>
#include <string>

namespace {

int failures = 0;

void expectEq(const char* label, const std::string& actual, const std::string& expected)
{
    if (actual != expected) {
        std::fprintf(stderr, "FAIL %s: got \"%s\", want \"%s\"\n", label,
                     actual.c_str(), expected.c_str());
        ++failures;
    } else {
        std::printf("ok   %s\n", label);
    }
}

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
    using namespace swap_ui;

    // --- unset: the compiled default ---
    expectEq("unset uses the compiled default", ethFaucetUrl("", false), kDefaultEthFaucetUrl);
    // The value the reviewer's `make faucet-poc-run` serves. If this ever
    // changes to a real deployment, the header comment above it must go too.
    expectEq("the compiled default is the local PoC service",
             kDefaultEthFaucetUrl, "http://127.0.0.1:8787");

    // --- set and empty: the off switch ---
    expectEq("an explicit empty value disables the faucet", ethFaucetUrl("", true), "");
    expectEq("a whitespace-only value disables the faucet", ethFaucetUrl("   \t\n", true), "");
    expect("a disabled faucet is not usable", isUsableFaucetUrl(ethFaucetUrl("", true)), false);

    // --- set to a URL ---
    expectEq("an override is used verbatim",
             ethFaucetUrl("https://faucet.example.org", true), "https://faucet.example.org");
    expectEq("surrounding whitespace is trimmed",
             ethFaucetUrl("  https://faucet.example.org  ", true), "https://faucet.example.org");
    // The client appends "/challenge" and "/drip"; a surviving trailing slash
    // would build "//challenge", which some proxies do not tolerate.
    expectEq("a trailing slash is dropped",
             ethFaucetUrl("https://faucet.example.org/", true), "https://faucet.example.org");
    expectEq("several trailing slashes are dropped",
             ethFaucetUrl("https://faucet.example.org///", true), "https://faucet.example.org");
    expectEq("a path is preserved",
             ethFaucetUrl("https://example.org/faucet/", true), "https://example.org/faucet");

    // --- usability ---
    expect("http is usable", isUsableFaucetUrl("http://127.0.0.1:8787"), true);
    expect("https is usable", isUsableFaucetUrl("https://faucet.example.org"), true);
    expect("the compiled default is usable", isUsableFaucetUrl(kDefaultEthFaucetUrl), true);
    // A schemeless paste must read as "no faucet", not as a URL that fails
    // later with a transport error nobody can act on.
    expect("a schemeless host is not usable", isUsableFaucetUrl("localhost:8787"), false);
    expect("a ws:// URL is not usable", isUsableFaucetUrl("ws://faucet.example.org"), false);
    expect("an empty string is not usable", isUsableFaucetUrl(""), false);

    if (failures != 0) {
        std::fprintf(stderr, "%d failure(s)\n", failures);
        return EXIT_FAILURE;
    }
    std::printf("ALL PASSED\n");
    return EXIT_SUCCESS;
}
