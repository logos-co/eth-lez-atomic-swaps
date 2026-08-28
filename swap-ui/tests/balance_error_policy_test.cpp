// Standalone unit test for swap-ui/src/balance_error_policy.h — the policy
// SwapUiPlugin::applyBalancesResult() uses instead of publishing every failed
// background balance read into `errorMessage`.
//
// The regression it pins (issue #169): a balance read nobody asked for must
// not raise an alarm on the first blip, must raise one per side once an
// endpoint is genuinely down, and must drop it the instant a read succeeds.
// The "which surface shows it" half lives in QML (SetupView's steps and the
// account strip above the Market board); this covers the decision itself.
//
//   c++ -std=c++17 -I../src -o /tmp/balance_error_policy_test balance_error_policy_test.cpp \
//     && /tmp/balance_error_policy_test

#include "../src/balance_error_policy.h"

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

// A real shape from the pinned Sepolia endpoint.
const std::string kTimeout =
    "error sending request for url (https://ethereum-sepolia-rpc.publicnode.com/): "
    "operation timed out";
const std::string kNotSetUp = "Ethereum key not set up yet";

} // namespace

int main()
{
    using namespace swap_ui;

    // --- Quiet on a blip -------------------------------------------------
    {
        BalanceSideErrors eth(BalanceSide::Eth);
        expect("healthy side says nothing", eth.message().empty());

        eth.recordFailure(kTimeout);
        expect("one failed read still says nothing", eth.message().empty());
        expect("but the failure is counted", eth.consecutiveFailures() == 1);

        eth.recordSuccess();
        expect("a success clears the streak", eth.consecutiveFailures() == 0);
        expect("and still says nothing", eth.message().empty());
    }

    // --- Loud on a dead endpoint -----------------------------------------
    {
        BalanceSideErrors eth(BalanceSide::Eth);
        eth.recordFailure(kTimeout);
        eth.recordFailure(kTimeout);
        expect("two consecutive failures publish a message", !eth.message().empty());
        expect("the message names Ethereum",
               eth.message().find("Ethereum") != std::string::npos);
        expect("the message never carries the raw RPC text",
               eth.message().find("operation timed out") == std::string::npos
                   && eth.message().find("http") == std::string::npos);

        // It stands until a read succeeds, rather than flickering off.
        eth.recordFailure(kTimeout);
        expect("a third failure keeps the message up", !eth.message().empty());
        expect("the streak does not grow without bound",
               eth.consecutiveFailures() == kFailuresBeforeVisible);

        eth.recordSuccess();
        expect("one good read clears the message", eth.message().empty());
        expect("and resets the streak", eth.consecutiveFailures() == 0);
    }

    // --- A blip does not accumulate across successes ----------------------
    {
        BalanceSideErrors lez(BalanceSide::Lez);
        for (int i = 0; i < 5; ++i) {
            lez.recordFailure(kTimeout);
            lez.recordSuccess();
        }
        expect("alternating fail/succeed never publishes", lez.message().empty());
    }

    // --- "Not set up yet" is not a failure --------------------------------
    {
        BalanceSideErrors eth(BalanceSide::Eth);
        eth.recordFailure(kNotSetUp);
        eth.recordFailure(kNotSetUp);
        eth.recordFailure(kNotSetUp);
        expect("a side that is not set up yet never alarms", eth.message().empty());
        expect("and does not accumulate a streak", eth.consecutiveFailures() == 0);

        // It also breaks a real streak rather than counting towards it: the
        // user regenerating a key mid-Setup is not evidence of a dead RPC.
        eth.recordFailure(kTimeout);
        eth.recordFailure(kNotSetUp);
        eth.recordFailure(kTimeout);
        expect("not-set-up between failures resets the streak", eth.message().empty());
    }

    // --- The two sides are independent, and say different things ----------
    {
        BalanceErrors errors;
        errors.eth.recordFailure(kTimeout);
        errors.eth.recordFailure(kTimeout);
        expect("a dead ETH endpoint alarms on ETH only", !errors.eth.message().empty());
        expect("the LEZ side stays quiet", errors.lez.message().empty());

        errors.lez.recordFailure(kTimeout);
        errors.lez.recordFailure(kTimeout);
        expect("the LEZ message names the LEZ network",
               errors.lez.message().find("LEZ") != std::string::npos);
        expect("the two sentences differ", errors.eth.message() != errors.lez.message());

        errors.reset();
        expect("reset clears ETH", errors.eth.message().empty());
        expect("reset clears LEZ", errors.lez.message().empty());
    }

    // --- A failed call counts against both sides --------------------------
    {
        BalanceErrors errors;
        errors.recordCallFailure(kTimeout);
        expect("one failed call is still a blip on ETH", errors.eth.message().empty());
        expect("one failed call is still a blip on LEZ", errors.lez.message().empty());

        errors.recordCallFailure(kTimeout);
        expect("a second failed call alarms ETH", !errors.eth.message().empty());
        expect("a second failed call alarms LEZ", !errors.lez.message().empty());
    }

    if (failures == 0) {
        std::printf("all balance_error_policy tests passed\n");
    }
    return failures == 0 ? 0 : 1;
}
