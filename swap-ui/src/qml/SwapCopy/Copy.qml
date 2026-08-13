pragma Singleton
import QtQuick

// The app's words for what the swap engine is doing.
//
// There were three independent step→English maps (the shell's status bar, the
// setup view, and the per-step `hint` text inside MakerView/TakerView), plus a
// separate vocabulary on the offer board. They disagreed: the same counterparty
// was a "maker" in the stepper and a "seller" in the status bar, and the same
// moment was "Waiting for LEZ Lock", "Waiting for the maker to detect your
// lock", and "Waiting for seller to lock LEZ" simultaneously.
//
// Vocabulary rules, applied throughout:
//   - The other party is the "seller" (they hold LEZ) or the "buyer" (they hold
//     ETH). Never "maker"/"taker" in user-facing text — those are protocol
//     roles, not words people use about money.
//   - The network layer is "the swap network". Never "Delivery", "messaging",
//     "fleet" or "infra".
//   - One loading verb per idea, and the ellipsis is always U+2026.
QtObject {
    id: copy

    // Short status line for a step. Falls back to a neutral phrase rather than
    // the raw backend enum: the old `map[step] || step` leaked identifiers like
    // "EthLockRejectedTakerAccount" onto the screen.
    function step(name) {
        var map = {
            // Seller (maker) side
            "WaitingForEthLock": "Waiting for a buyer to lock their ETH…",
            "EthLockDetected":   "A buyer locked their ETH",
            "EthLockRejected":   "That buyer's timer was too short — still waiting",
            "EthLockRejectedTakerAccount":
                                 "That buyer isn't the one you designated — still waiting",
            "LezLocking":        "Locking your LEZ in escrow…",
            "LezLocked":         "Your LEZ is locked",
            "WaitingForPreimage": "Waiting for the buyer to claim…",
            "PreimageRevealed":  "The buyer claimed — collecting your ETH next",
            "ClaimingEth":       "Collecting your ETH…",
            "EthClaimed":        "ETH collected",

            // Buyer (taker) side
            "PreimageGenerated": "Secret generated",
            "LockingEth":        "Locking your ETH…",
            "EthLocked":         "Your ETH is locked",
            "WaitingForLezLock": "Waiting for the seller to lock their LEZ…",
            "LezLockDetected":   "The seller locked their LEZ",
            "VerifyingLezEscrow": "Checking the seller's escrow…",
            "LezEscrowVerified": "The escrow checks out",
            "ClaimingLez":       "Claiming your LEZ…",
            "LezClaimed":        "LEZ claimed",

            // Recovery
            "TimelockExpired":   "The timer ran out — your funds can be claimed back",
            "Refunding":         "Returning your funds…",
            "RefundComplete":    "Your funds are back",

            // Selling loop
            "AutoAcceptStarted": "Going live…",
            "AutoAcceptCancelled": "Stopped",
            "AutoAcceptStopped": "Stopped",
            "AutoAcceptTransientError": "Hit a snag — retrying…",
            "AutoAcceptSwapFailed": "That swap didn't complete",
            "AutoAcceptSwapCompleted": "Swap complete",
        }
        return map[name] || "Working…"
    }

    // The reassurance motif, in one place so it cannot drift.
    readonly property string fundsSafeGeneral:
        "A stalled swap cannot lose your funds. Both sides are locked with a "
        + "timer, and when it runs out each side can take its own money back."
    readonly property string ethAutoRefunds:
        "Your ETH is safe: if the seller never completes, it comes back to you "
        + "automatically once the timer runs out."
    readonly property string lezAutoRefunds:
        "Your LEZ is safe: if the buyer never claims, you can take it back once "
        + "the timer runs out."
    readonly property string nothingLockedYet:
        "Nothing has left your wallet yet."
}
