import QtQuick
import QtQuick.Layouts
import SwapTheme
import SwapLinks
import SwapFormat
import SwapCopy

// Your swap in progress, and its receipt.
//
// This view used to contain a SECOND copy of the market: its own offer merge
// off the offersFetched signal, its own "Discover Offers" button, its own tape
// rows, its own empty states, and its own confirm-purchase card. The Market tab
// already did all of that, better, against a keyed model — so the app had two
// half-markets and no clear answer to where you browse. All of it is gone.
//
// One job now: show the swap you started from Market, then its receipt. Offers
// are chosen on Market; this screen is what happens next.
PageScaffold {
    id: takerRoot

    title: "Your swap"
    subtitle: takerRoot.hasActivity
              ? ""
              : "Pick an offer on the Market tab and it runs here."

    // Set by the shell when the Market board accepts an offer (OfferBoard emits
    // swapEngaged/swapAbandoned; AtomicSwapView wires them to these).
    property var acceptedOffer: null
    property bool swapCompleted: false

    signal browseMarket()

    // Each step carries a short `hint` — a calm expectation line the stepper
    // shows under the active step. The multi-minute waits (steps 4/5) are NORMAL
    // on the LEZ test network, so the copy says so and reminds the user their
    // locked ETH comes back if the seller never completes.
    property var takerSteps: [
        { name: "PreimageGenerated", label: "Generate your secret",
          hint: "Making the secret that locks this swap. " + Copy.nothingLockedYet },
        { name: "LockingEth",        label: "Lock your ETH",
          hint: "Locking your ETH on Ethereum — usually under a minute." },
        { name: "EthLocked",         label: "Your ETH is locked",
          hint: "Locked. If the seller never locks their LEZ, it comes back to "
                + "you automatically after the timer — a stalled swap cannot "
                + "lose your funds." },
        { name: "WaitingForLezLock", label: "Wait for the seller",
          hint: "Waiting for the seller to notice your lock and lock their LEZ. "
                + "Typically 1–5 minutes — LEZ blocks can be a minute or more "
                + "apart, so a wait here is normal, not stuck. If they never "
                + "lock, your ETH comes back after the timer." },
        { name: "LezLockDetected",   label: "Seller locked their LEZ",
          hint: "The seller locked. Checking their escrow matches your swap." },
        { name: "VerifyingLezEscrow", label: "Check the escrow",
          hint: "Checking the seller's escrow — amount, secret and timer. "
                + "Usually seconds." },
        { name: "LezEscrowVerified", label: "Escrow checks out",
          hint: "All good. Claiming your LEZ next." },
        { name: "ClaimingLez",       label: "Claim your LEZ",
          hint: "Claiming your LEZ by revealing the secret — typically 1–5 "
                + "minutes for the LEZ network to confirm." },
        { name: "LezClaimed",        label: "LEZ claimed",
          hint: "" },
    ]

    // Steps where the buyer's ETH is locked and the seller's completion is still
    // pending — the window where "am I stuck?" bites and where the auto-refund
    // reassurance plus the on-chain lock proof are worth surfacing.
    readonly property var ethLockedSteps: [
        "EthLocked", "WaitingForLezLock", "LezLockDetected",
        "VerifyingLezEscrow", "LezEscrowVerified", "ClaimingLez"
    ]

    property var completedSteps: {
        var done = []
        var steps = swapBackend.takerProgressSteps
        for (var i = 0; i < steps.length; i++) {
            if (steps[i] !== swapBackend.takerCurrentStep && done.indexOf(steps[i]) < 0)
                done.push(steps[i])
        }
        return done
    }

    readonly property bool hasActivity: swapBackend.takerRunning
                                        || takerRoot.swapCompleted
                                        || takerRoot.acceptedOffer !== null

    // Receipt context, captured in QML at the run boundaries (session-only):
    // the accepted offer is snapshotted *before* it is cleared at completion,
    // and wall-clock stamps bracket the run.
    property var completedOffer: null
    property double swapStartedMs: 0
    property double swapFinishedMs: 0

    function latestReceiptForRole(receiptsJson, wantedRole) {
        try {
            var receipts = JSON.parse(receiptsJson || "[]")
            for (var i = 0; i < receipts.length; i++) {
                if (receipts[i].role === wantedRole)
                    return receipts[i]
            }
        } catch (e) {
            // A malformed historic line must not hide the live result card.
        }
        return null
    }

    function publicReceiptContext(receipt) {
        var safeReceipt = receipt || {}
        var eth = safeReceipt.eth || {}
        var network = safeReceipt.network || {}
        return {
            ethLockTx: String(eth.lock_tx || ""),
            network: {
                lezSequencer: String(network.lez_sequencer || ""),
                ethChainId: Number(network.eth_chain_id || 0)
            }
        }
    }

    readonly property var latestTakerReceipt: latestReceiptForRole(
        swapBackend.receiptsJson, "taker")

    readonly property var receiptContext: {
        var ctx = {
            startedMs: takerRoot.swapStartedMs,
            finishedMs: takerRoot.swapFinishedMs
        }
        // The backend journals the completed receipt before publishing the
        // result/running signals. Reuse that authoritative record so the
        // immediate post-swap card has the same ETH lock proof and network facts
        // as History, instead of waiting for the user to change tabs.
        var publicReceipt = takerRoot.publicReceiptContext(
            takerRoot.latestTakerReceipt)
        ctx.ethLockTx = publicReceipt.ethLockTx
        ctx.network = publicReceipt.network
        var offer = takerRoot.completedOffer
        if (offer) {
            ctx.lezAmount = String(offer.lez_amount || "")
            ctx.ethAmountWei = String(offer.eth_amount || "")
            ctx.counterpartyEth = String(offer.maker_eth_address || "")
            ctx.counterpartyLez = String(offer.maker_lez_account || "")
            ctx.ethHtlcAddress = String(offer.eth_htlc_address || "")
            ctx.lezProgramId = String(offer.lez_htlc_program_id || "")
            ctx.lezTimelockUnix = Number(offer.lez_timelock || 0)
            ctx.ethTimelockUnix = Number(offer.eth_timelock || 0)
        }
        return ctx
    }

    Connections {
        target: swapBackend
        function onTakerRunningChanged() {
            if (swapBackend.takerRunning) {
                // A run is starting (offer swap or ETH refund): stamp the wall
                // clock and drop any stale snapshot so the receipt never mixes
                // contexts.
                takerRoot.swapStartedMs = Date.now()
                takerRoot.swapFinishedMs = 0
                takerRoot.completedOffer = null
                return
            }
            takerRoot.swapFinishedMs = Date.now()
            if (takerRoot.acceptedOffer !== null) {
                // Snapshot the offer context *before* clearing it — the receipt
                // renders amounts, counterparty and timelocks from it.
                takerRoot.completedOffer = takerRoot.acceptedOffer
                takerRoot.swapCompleted = true
                takerRoot.acceptedOffer = null
            }
        }
    }

    // --- Nothing running --------------------------------------------------
    Card {
        visible: !takerRoot.hasActivity

        EmptyState {
            Layout.fillWidth: true
            tone: Theme.toneWaiting
            title: "No swap running"
            subtitle: "When you accept an offer on the Market tab, it appears here "
                      + "and runs step by step."

            PrimaryButton {
                text: "Browse the market"
                Layout.preferredWidth: 220
                Layout.preferredHeight: 42
                onClicked: takerRoot.browseMarket()
            }
        }
    }

    // --- What you're buying ------------------------------------------------
    Card {
        visible: swapBackend.takerRunning && takerRoot.acceptedOffer !== null
        tone: "active"

        SectionHeader {
            label: "Buying"
            hairline: false
        }
        Text {
            text: takerRoot.acceptedOffer
                  ? takerRoot.acceptedOffer.lez_amount + " LEZ for "
                    + Format.weiToEth(takerRoot.acceptedOffer.eth_amount)
                  : ""
            color: Theme.accent
            font.pixelSize: Theme.fontLarge
            font.bold: true
        }
        HexValue {
            visible: takerRoot.acceptedOffer !== null
            label: "Seller"
            value: takerRoot.acceptedOffer
                   ? (takerRoot.acceptedOffer.maker_eth_address || "") : ""
        }
    }

    // --- Progress ----------------------------------------------------------
    Card {
        visible: swapBackend.takerRunning

        SectionHeader {
            label: "Swap in progress"
            hairline: false
        }
        ProgressStepper {
            id: takerStepper
            Layout.fillWidth: true
            steps: takerRoot.takerSteps
            currentStep: swapBackend.takerCurrentStep
            completedSteps: takerRoot.completedSteps
        }

        // Auto-refund reassurance, with a live countdown to the ETH timelock.
        // The accepted offer carries the absolute eth_timelock the buyer adopts
        // at lock time, so this is the real deadline. Re-evaluated every second
        // by reading the stepper's ticking elapsed counter as a dependency — no
        // extra Timer.
        //
        // Note this only renders while `acceptedOffer` is set, which is
        // session-only state: restart the app mid-swap and the deadline is
        // unknown. In that case the general reassurance below still shows —
        // what must never happen is inventing a countdown we cannot back.
        SafetyNote {
            visible: takerRoot.ethLockedSteps.indexOf(swapBackend.takerCurrentStep) >= 0
            text: Copy.ethAutoRefunds
            deadline: {
                var _tick = takerStepper.activeElapsedSeconds // 1s refresh dep
                if (takerRoot.acceptedOffer === null) return ""
                var remaining = Format.expiresIn(takerRoot.acceptedOffer.eth_timelock)
                if (remaining === "" || remaining === "expired") return ""
                return "That's about " + remaining + " from now."
            }
        }

        // On-chain lock proof, surfaced mid-swap so a user can check the tx
        // themselves instead of wondering whether their lock landed.
        HexValue {
            visible: swapBackend.takerEthLockTx !== ""
            Layout.topMargin: Theme.spacingSmall
            label: "Your ETH lock"
            value: swapBackend.takerEthLockTx
            link: Links.ethTx(swapBackend.takerEthLockTx, swapBackend.takerEthChainId)
        }
    }

    // --- Receipt ------------------------------------------------------------
    ReceiptCard {
        Layout.fillWidth: true
        role: "taker"
        resultJson: swapBackend.takerResultJson
        context: takerRoot.receiptContext
    }

    GhostButton {
        visible: takerRoot.swapCompleted && !swapBackend.takerRunning
        text: "Browse the market"
        Layout.fillWidth: true
        Layout.preferredHeight: 42
        font.bold: true
        onClicked: {
            takerRoot.swapCompleted = false
            takerRoot.acceptedOffer = null
            takerRoot.browseMarket()
        }
    }
}
