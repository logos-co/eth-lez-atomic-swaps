// Standalone unit test for swap-ui/src/lez_activation.h — what the Setup
// page concludes from one attempt at activating a LEZ account (issue #171).
//
// The load-bearing cases are the ones the captain hit live: an EMPTY result
// (the async wrapper's 20s Timeout expiring on a call that blocks up to 300s
// and then succeeded on-chain) must NOT read as a failure, and no failure may
// ever be reported with a blank detail.
//
// Deliberately has zero Qt/logos/nix dependencies so it can be compiled and
// run directly:
//
//   c++ -std=c++17 -I../src -o /tmp/lez_activation_test lez_activation_test.cpp \
//     && /tmp/lez_activation_test
//
// Exits 0 and prints "ALL PASSED" on success; exits non-zero with a message
// per failing case otherwise.

#include "../src/lez_activation.h"

#include <cstdio>
#include <cstdlib>
#include <string>

namespace {

int failures = 0;

const char* name(swap_ui::ActivationVerdict v)
{
    switch (v) {
    case swap_ui::ActivationVerdict::Succeeded: return "Succeeded";
    case swap_ui::ActivationVerdict::Retry:     return "Retry";
    case swap_ui::ActivationVerdict::Failed:    return "Failed";
    }
    return "?";
}

void expectVerdict(const char* label, swap_ui::ActivationVerdict actual,
                   swap_ui::ActivationVerdict expected)
{
    if (actual != expected) {
        std::fprintf(stderr, "FAIL %s: got %s, want %s\n", label, name(actual), name(expected));
        ++failures;
    } else {
        std::printf("ok   %s\n", label);
    }
}

void expectTrue(const char* label, bool actual)
{
    if (!actual) {
        std::fprintf(stderr, "FAIL %s\n", label);
        ++failures;
    } else {
        std::printf("ok   %s\n", label);
    }
}

void expectEq(const char* label, const std::string& actual, const std::string& expected)
{
    if (actual != expected) {
        std::fprintf(stderr, "FAIL %s: got \"%s\", want \"%s\"\n", label, actual.c_str(),
                     expected.c_str());
        ++failures;
    } else {
        std::printf("ok   %s\n", label);
    }
}

} // namespace

int main()
{
    using swap_ui::ActivationVerdict;
    using swap_ui::classifyActivation;

    const std::string ok1 = R"({"outcome":"Initialized","data":{"tx_hash":"ab"}})";

    // --- Rule 1: only a recognised outcome activates an account. ---
    {
        const auto d = classifyActivation("", "Initialized", ok1, false);
        expectVerdict("\"Initialized\" -> Succeeded", d.verdict, ActivationVerdict::Succeeded);
        expectEq("Succeeded carries the outcome", d.outcome, "Initialized");
        expectTrue("Succeeded has no detail", d.detail.empty());
    }
    {
        const auto d = classifyActivation("", "AlreadyInitialized", R"({"outcome":"AlreadyInitialized"})", true);
        expectVerdict("\"AlreadyInitialized\" -> Succeeded (even on the re-check)",
                      d.verdict, ActivationVerdict::Succeeded);
        expectEq("re-check carries the outcome", d.outcome, "AlreadyInitialized");
    }
    expectVerdict("unrecognised outcome never succeeds",
                  classifyActivation("", "Submitted", R"({"outcome":"Submitted"})", true).verdict,
                  ActivationVerdict::Failed);
    expectVerdict("empty outcome never succeeds",
                  classifyActivation("", "", "{}", true).verdict, ActivationVerdict::Failed);
    // An error field beside a good-looking outcome is authoritative: a clean
    // recognised success is the only thing that may mark an account activated.
    expectVerdict("error alongside a recognised outcome never succeeds",
                  classifyActivation("boom", "Initialized", "{}", true).verdict,
                  ActivationVerdict::Failed);

    // --- Rule 2: inconclusive is not failure — re-check first. ---
    //
    // This is issue #171 exactly: the async wrapper's default 20s Timeout
    // yields an invalid QVariant that becomes an EMPTY QString, while the
    // 300s call goes on to succeed on-chain.
    expectVerdict("empty result (#171) -> Retry, not Failed",
                  classifyActivation("", "", "", false).verdict, ActivationVerdict::Retry);
    expectVerdict("whitespace-only result -> Retry",
                  classifyActivation("", "", "  \n ", false).verdict, ActivationVerdict::Retry);
    expectVerdict("unrecognised shape -> Retry first",
                  classifyActivation("", "", R"({"nope":1})", false).verdict,
                  ActivationVerdict::Retry);
    // A real error re-checks too. "did not commit within 300s" is the case
    // MOST likely to have landed by the time we ask again.
    expectVerdict("commit-timeout error -> Retry first",
                  classifyActivation("account Initialize did not commit within 300s (tx ab)", "",
                                     R"({"error":"..."})", false)
                      .verdict,
                  ActivationVerdict::Retry);
    // ...and exactly once. The re-check is the last word, so this pair can
    // never loop.
    expectVerdict("a re-checked inconclusive answer is Failed",
                  classifyActivation("", "", "", true).verdict, ActivationVerdict::Failed);
    expectTrue("Retry carries no detail and no outcome", [] {
        const auto d = classifyActivation("", "", "", false);
        return d.detail.empty() && d.outcome.empty();
    }());

    // --- Rule 3: a failure always says something. ---
    {
        const auto d = classifyActivation("", "", "", true);
        expectEq("empty-everything failure uses the lost-contact sentence", d.detail,
                 swap_ui::kActivationLostContact);
        expectTrue("lost-contact sentence does not claim failure",
                   d.detail.find("didn't work") == std::string::npos
                       && d.detail.find("failed") == std::string::npos);
    }
    expectEq("an error field is reported verbatim",
             classifyActivation("invalid sequencer URL", "", R"({"error":"x"})", true).detail,
             "invalid sequencer URL");
    expectEq("an unrecognised payload is shown, never a bare colon",
             classifyActivation("", "", R"({"nope":1})", true).detail,
             R"(Unexpected activation result: {"nope":1})");
    // The regression itself: this exact call used to produce
    // "Unexpected activation result: " with nothing after the colon.
    for (const char* raw : {"", " ", "\n", "\t\r\n "}) {
        const auto d = classifyActivation("", "", raw, true);
        expectTrue("blank payload never yields an empty detail", !d.detail.empty());
        expectTrue("blank payload never yields a bare \"Unexpected activation result: \"",
                   d.detail != "Unexpected activation result: "
                       && d.detail.rfind("Unexpected activation result:", 0) != 0);
    }
    // Belt and braces over the whole matrix: Failed always has a detail,
    // Succeeded never does, and only Succeeded names an outcome.
    for (const char* err : {"", " ", "boom"}) {
        for (const char* out : {"", "Initialized", "AlreadyInitialized", "Weird"}) {
            for (const char* raw : {"", "{}", R"({"outcome":"Weird"})"}) {
                for (bool rechecked : {false, true}) {
                    const auto d = classifyActivation(err, out, raw, rechecked);
                    if (d.verdict == ActivationVerdict::Failed && d.detail.empty()) {
                        std::fprintf(stderr,
                                     "FAIL Failed with empty detail (err=\"%s\" out=\"%s\" "
                                     "raw=\"%s\" rechecked=%d)\n",
                                     err, out, raw, rechecked ? 1 : 0);
                        ++failures;
                    }
                    if (d.verdict != ActivationVerdict::Failed && !d.detail.empty()) {
                        std::fprintf(stderr, "FAIL non-Failed carries a detail (%s)\n",
                                     name(d.verdict));
                        ++failures;
                    }
                    if (d.outcome.empty() != (d.verdict != ActivationVerdict::Succeeded)) {
                        std::fprintf(stderr, "FAIL outcome set iff Succeeded (%s)\n",
                                     name(d.verdict));
                        ++failures;
                    }
                }
            }
        }
    }
    std::printf("ok   matrix: Failed always explains itself, only Succeeded names an outcome\n");

    // --- The transport budget has to outlast what the call can take. ---
    // INIT_COMMIT_TIMEOUT is 300s (src/lez/onboard.rs); anything at or below
    // it reintroduces issue #171 by abandoning a call that is still waiting
    // for a block.
    expectTrue("activation timeout exceeds the 300s commit wait",
               swap_ui::kActivationTimeoutMs > 300 * 1000);
    expectTrue("the phase flip happens well before the call can finish",
               swap_ui::kActivationAwaitCommitMs > 0
                   && swap_ui::kActivationAwaitCommitMs < swap_ui::kActivationTimeoutMs);
    // The two UI-only step names must not collide with a FundingProgress
    // variant, or the funding job's progress would drive them.
    expectTrue("UI phase names are distinct from the outcome names",
               std::string(swap_ui::kStepAwaitingCommit) != swap_ui::kOutcomeInitialized
                   && std::string(swap_ui::kStepAwaitingCommit) != swap_ui::kOutcomeAlreadyInitialized
                   && std::string(swap_ui::kStepVerifying) != swap_ui::kOutcomeInitialized
                   && std::string(swap_ui::kStepVerifying) != swap_ui::kOutcomeAlreadyInitialized);

    if (failures != 0) {
        std::fprintf(stderr, "%d failure(s)\n", failures);
        return EXIT_FAILURE;
    }
    std::printf("ALL PASSED\n");
    return EXIT_SUCCESS;
}
