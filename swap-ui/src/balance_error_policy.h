#ifndef SWAP_UI_BALANCE_ERROR_POLICY_H
#define SWAP_UI_BALANCE_ERROR_POLICY_H

#include <string>

#include "balance_read_gate.h"

// Pure, dependency-free policy for what a FAILED BALANCE READ is allowed to
// say, shared between the plugin (swap_ui_plugin.cpp, applyBalancesResult) and
// its unit test (tests/balance_error_policy_test.cpp). No Qt, so the test
// compiles standalone (see `make swap-ui-unit`).
//
// Issue #169: every caller of fetchBalances() is AUTOMATIC — the Market view's
// timers, the Setup tab's Sepolia-arrival poll, the post-key-generation
// refreshes and the post-swap settle poll. There is no user-initiated refresh
// (the Refresh button was removed). So publishing a failed read into
// `errorMessage` put a global red banner across the top of the app for
// something the user never asked for: a fresh install still on Setup step 2
// got a full-width alarm because one background Sepolia read timed out.
//
// Routing those to the status line instead would be the other wrong answer —
// a genuinely dead RPC would then show up nowhere the user is looking, which
// during a public trial reads as "nothing works and I cannot tell why".
//
// So a balance failure is published PER SIDE, and the views put it where the
// user is already looking: the Setup step that needs that chain, and the
// account strip above the Market board. Two rules keep it proportionate:
//
//   - QUIET on a blip. One failed read publishes nothing. Public endpoints
//     drop single requests routinely (measured against both Sepolia and
//     testnet.lez.logos.co), and the callers retry on their own within
//     seconds; an alarm that resolves itself before it is read is noise.
//   - LOUD on a dead endpoint. After `kFailuresBeforeVisible` consecutive
//     failures of the SAME side the message stands until a read succeeds, so
//     an endpoint that is really down is discoverable rather than silent.
//
// A side that is simply not set up yet is not a failure at all and never
// counts (`sideNotSetUp`, balance_read_gate.h) — that is the expected shape
// mid-Setup, where the ETH key exists and the LEZ account does not.
namespace swap_ui {

// Consecutive failed reads of one side before its message becomes visible.
// Two, not one: the Setup poll and the Market timer both retry within seconds,
// so the second failure is the first evidence of something persistent.
inline constexpr int kFailuresBeforeVisible = 2;

// Which chain a message is about. Named rather than baked into the text so the
// two sentences below cannot drift apart from the sides they describe.
enum class BalanceSide { Eth, Lez };

// The user-facing sentence for a failed read of `side`. Deliberately does NOT
// carry the raw RPC text: "error sending request for url (...): operation
// timed out" in a first-run UI reads as a crash, and the raw error is never
// discarded anyway — it stays verbatim in `lastResultJson` and the trace log
// for diagnostics. Says which chain, says what it means for what is on screen,
// and says the app will keep trying, because it does. Worded to read correctly
// in every place it is shown — the Setup step for that chain, and the strip
// under the account balances — so the two surfaces cannot drift apart.
inline std::string balanceErrorCopy(BalanceSide side)
{
    return side == BalanceSide::Eth
        ? "Can't reach Ethereum right now, so the ETH balance may be out of "
          "date. The app keeps trying."
        : "Can't reach the LEZ network right now, so the LEZ balance may be "
          "out of date. The app keeps trying.";
}

// One chain's running read state. `message` is what the views should show:
// empty means "say nothing", which covers both a healthy side and a side whose
// single blip has not earned an alarm yet.
class BalanceSideErrors {
public:
    explicit BalanceSideErrors(BalanceSide side) : m_side(side) {}

    // A read of this side succeeded. Clears everything immediately — one good
    // read is proof the endpoint is back, and leaving a stale alarm up next to
    // a fresh balance is worse than never showing it.
    void recordSuccess()
    {
        m_consecutiveFailures = 0;
        m_message.clear();
    }

    // A read of this side failed with `rawError`. A "not set up yet" result is
    // not a failure: it is the gate reporting a side it deliberately skipped,
    // so it clears the streak exactly like a success would rather than slowly
    // accumulating into an alarm while the user works through Setup.
    void recordFailure(const std::string& rawError)
    {
        if (sideNotSetUp(rawError)) {
            recordSuccess();
            return;
        }
        if (m_consecutiveFailures < kFailuresBeforeVisible) {
            ++m_consecutiveFailures;
        }
        m_message = m_consecutiveFailures >= kFailuresBeforeVisible
            ? balanceErrorCopy(m_side)
            : std::string{};
    }

    const std::string& message() const { return m_message; }
    int consecutiveFailures() const { return m_consecutiveFailures; }

    // A new endpoint or account is a new chance: drop the history rather than
    // holding the previous URL's failures against it.
    void reset() { recordSuccess(); }

private:
    BalanceSide m_side;
    int m_consecutiveFailures = 0;
    std::string m_message;
};

// Both sides together, in the shape applyBalancesResult consumes them.
struct BalanceErrors {
    BalanceSideErrors eth{BalanceSide::Eth};
    BalanceSideErrors lez{BalanceSide::Lez};

    void reset()
    {
        eth.reset();
        lez.reset();
    }

    // A whole-result error (the call itself failed, so neither side was read)
    // counts against BOTH sides — there is no per-side detail to split it by,
    // and a transport failure means every balance on screen is stale.
    void recordCallFailure(const std::string& rawError)
    {
        eth.recordFailure(rawError);
        lez.recordFailure(rawError);
    }
};

} // namespace swap_ui

#endif // SWAP_UI_BALANCE_ERROR_POLICY_H
