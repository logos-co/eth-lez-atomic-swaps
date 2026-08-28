// What the Setup page should conclude from one attempt at activating a LEZ
// account — the pure decision behind SwapUiPlugin::setupInitializeAccount.
//
// Why this exists (issue #171). `swap_ffi_lez_ensure_initialized` BLOCKS: it
// submits `Initialize` and then polls until ownership actually flips, for up
// to INIT_COMMIT_TIMEOUT = 300s (src/lez/onboard.rs), because public-testnet
// blocks can be a minute or more apart. The generated `…Async` wrapper's
// default `Timeout` is 20s (logos_mode.h), and on expiry it hands the callback
// an INVALID QVariant, which its own lambda turns into an EMPTY QString. The
// plugin then read that empty string as a result: `jsonError("")` is "" and
// `parseObject("")` is `{}`, so it fell straight through to
// "Unexpected activation result: " — a failure claimed 20s into an activation
// that went on to succeed on-chain, with a blank detail after the colon. The
// retry then said "Account already set up", which is how the captain found it.
//
// Three rules come out of that, and they are what this header encodes:
//
//   1. The strict outcome check STAYS. Only "Initialized"/"AlreadyInitialized"
//      may mark an account activated. Guessing wrong is worse than an honest
//      retry: the sequencer SILENTLY DROPS actions against a never-initialized
//      account, so a false "done" here surfaces much later as a swap that
//      simply vanished.
//   2. An inconclusive answer is not a failure. "No answer" and "it failed"
//      are different things, and the whole bug was calling the first the
//      second. Anything that is not a recognised success re-checks once
//      before anything is declared failed — a re-check is nearly free for an
//      already-initialized account (ensure_initialized re-reads ownership
//      before submitting anything, and is documented safe to call repeatedly
//      for the same signer from a single caller).
//   3. A failure always says something. `detail` is never empty when the
//      verdict is Failed, so the UI can never render a bare
//      "Unexpected activation result: " again.
//
// Deliberately has zero Qt/logos/nix dependencies so it compiles in the
// plain-C++ swap-ui unit harness:
//
//   c++ -std=c++17 -Iswap-ui/src swap-ui/tests/lez_activation_test.cpp && ./a.out

#pragma once

#include <string>

namespace swap_ui {

// The two outcomes `ensure_initialized` reports for a usable account. Kept as
// strings because that is the wire shape (InitOutcome's serde tag), and the
// funding job reports the same two names as progress steps.
inline constexpr const char* kOutcomeInitialized = "Initialized";
inline constexpr const char* kOutcomeAlreadyInitialized = "AlreadyInitialized";

// setupStep values that exist only to give the blocking call a visible phase.
// Neither is a FundingProgress variant, so they cannot collide with the
// funding job's own progress steps.
//
// The call reports nothing at all until it answers, so the UI would otherwise
// sit on "Initializing" for up to five minutes and look hung. Once the elapsed
// time is past anything a submission plausibly takes, the honest thing to say
// is that we are now waiting on the chain, not on the app.
inline constexpr const char* kStepAwaitingCommit = "AwaitingCommit";
// Shown while rule 2's re-check is in flight.
inline constexpr const char* kStepVerifying = "Verifying";

// How long to let the whole call run before the transport gives up on it.
// Must exceed INIT_COMMIT_TIMEOUT (300s) or we are back to issue #171,
// abandoning an activation that is still legitimately waiting for a block.
inline constexpr int kActivationTimeoutMs = 330 * 1000;
// How long "Initializing" is a fair description before the phase becomes
// "waiting for the network to confirm".
inline constexpr int kActivationAwaitCommitMs = 12 * 1000;

// What to tell a user when the app never learned what happened. Deliberately
// does not claim failure — the transaction may well be committing right now,
// which is exactly the case that produced issue #171.
inline constexpr const char* kActivationLostContact =
    "The app didn't get an answer back while activating your account, so it "
    "can't tell you whether the transaction landed. Nothing is lost either "
    "way. Wait a moment and press Activate account again — if it did land, "
    "that check confirms it in a second without sending anything.";

enum class ActivationVerdict {
    // A recognised initialized outcome: mark the account activated.
    Succeeded,
    // Inconclusive, and no re-check has run yet. Ask again before judging.
    Retry,
    // Conclusive, and `detail` explains it in a non-empty sentence.
    Failed,
};

struct ActivationDecision {
    ActivationVerdict verdict;
    // The outcome name to show as the step, set only when Succeeded.
    std::string outcome;
    // Never empty when Failed; empty otherwise.
    std::string detail;
};

inline bool isInitializedOutcome(const std::string& outcome)
{
    return outcome == kOutcomeInitialized || outcome == kOutcomeAlreadyInitialized;
}

inline bool isBlank(const std::string& value)
{
    return value.find_first_not_of(" \t\r\n") == std::string::npos;
}

// Decide what one activation attempt means.
//
//   error     the result's "error" field ("" when absent)
//   outcome   the result's "outcome" field ("" when absent)
//   raw       the whole result payload, so an unrecognised shape can be shown
//   rechecked whether this attempt WAS the one re-check (rule 2), i.e. no
//             further attempt will be made on this button press
//
// The caller does the JSON; this does the judging.
inline ActivationDecision classifyActivation(const std::string& error,
                                             const std::string& outcome,
                                             const std::string& raw,
                                             bool rechecked)
{
    // An explicit error field is authoritative even if an outcome rode along
    // beside it: rule 1 says only a clean recognised success counts.
    if (isBlank(error) && isInitializedOutcome(outcome)) {
        return {ActivationVerdict::Succeeded, outcome, {}};
    }
    if (!rechecked) {
        // Rule 2. Includes the empty payload that caused #171, an
        // unrecognised shape, and a real error — a "did not commit within
        // 300s" error in particular is the case most likely to have landed by
        // the time we ask again.
        return {ActivationVerdict::Retry, {}, {}};
    }
    // Rule 3: whatever we say now, say something.
    if (!isBlank(error)) {
        return {ActivationVerdict::Failed, {}, error};
    }
    if (!isBlank(raw)) {
        return {ActivationVerdict::Failed, {},
                "Unexpected activation result: " + raw};
    }
    return {ActivationVerdict::Failed, {}, kActivationLostContact};
}

} // namespace swap_ui
