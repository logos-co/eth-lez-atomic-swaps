import QtQuick
import QtQuick.Layouts
import SwapTheme
import SwapFormat
import SwapCopy

// Sell LEZ — publish an offer and let buyers take it.
//
// Three things changed here beyond styling:
//
//  - The live step is shown on this page. It used to live only in the shell's
//    bottom status bar, ~900px below the stepper describing the same moment.
//  - The seller now gets the safety reassurance too. Their LEZ sits in escrow
//    for minutes at a time and nothing on this screen ever said it comes back.
//  - The per-sale receipt is rendered. buildLoopReceipt() has always run on
//    every completed sale and the result was assigned to `loopReceipt` and then
//    never displayed — the evidence was assembled and thrown away.
PageScaffold {
    id: makerRoot

    title: "Sell LEZ"
    subtitle: "Publish your rate as an offer. Buyers lock ETH, you lock LEZ, and "
              + "the swap finishes on its own."

    headerTrailingData: StatusChip {
        text: swapBackend.autoAcceptRunning ? "Live" : "Offline"
        status: swapBackend.autoAcceptRunning ? "live" : "waiting"
        bold: swapBackend.autoAcceptRunning
    }

    // Hints render as one calm expectation line under the active step (shared
    // ProgressStepper). The seller's waits (for the buyer's ETH lock, then for
    // the buyer's claim) are the ones that look idle for minutes.
    property var makerSteps: [
        { name: "WaitingForEthLock", label: "Wait for the buyer to lock ETH",
          hint: "Listening for a buyer to take your offer. This can sit idle for "
                + "a long time — that is normal, not stuck." },
        { name: "EthLockDetected",   label: "Buyer locked ETH",
          hint: "A buyer locked their ETH. Getting ready to lock your LEZ." },
        { name: "LezLocking",        label: "Lock your LEZ",
          hint: "Locking your LEZ in escrow — LEZ blocks can be a minute or more "
                + "apart, so a short wait here is normal." },
        { name: "LezLocked",         label: "Your LEZ is locked",
          hint: "Locked. Waiting for the buyer to claim it, which reveals the "
                + "secret you need to collect their ETH." },
        { name: "WaitingForPreimage", label: "Wait for the buyer to claim",
          hint: "Waiting for the buyer to claim their LEZ. Typically 1–5 minutes. "
                + "If they never do, your LEZ comes back to you after the timer." },
        { name: "PreimageRevealed",  label: "Buyer claimed",
          hint: "Got what we need. Collecting the buyer's ETH." },
        { name: "ClaimingEth",       label: "Collect your ETH",
          hint: "Collecting the buyer's ETH on Ethereum — usually under a minute." },
        { name: "EthClaimed",        label: "ETH collected",
          hint: "" },
    ]

    // Track completed steps based on progress events
    property var completedSteps: {
        var done = []
        var steps = swapBackend.makerProgressSteps
        for (var i = 0; i < steps.length; i++) {
            if (steps[i] !== swapBackend.makerCurrentStep && done.indexOf(steps[i]) < 0)
                done.push(steps[i])
        }
        return done
    }

    property string cumulativeStats: {
        var n = swapBackend.autoAcceptCompleted
        if (n <= 0) return ""
        var lezSold = n * Number(swapBackend.lezAmount)
        var ethEarned = n * Number(swapBackend.ethAmount)
        return n + " swap" + (n > 1 ? "s" : "") + " completed — "
               + lezSold + " LEZ sold for " + ethEarned + " ETH"
    }

    readonly property bool inSwap: swapBackend.autoAcceptRunning
                                   && swapBackend.makerCurrentStep !== ""
                                   && swapBackend.makerCurrentStep !== "WaitingForEthLock"

    // --- Receipt capture (session-only, PR1) ---------------------------
    // The auto-accept loop reports per-swap completions without a result JSON
    // (the backend discards the outcome until a later PR), so the receipt is
    // assembled from what is in memory at completion time: config
    // amounts/contracts/timelocks plus the taker's Delivery SwapAccept
    // (hashlock, ETH swap ID, taker identities) surfaced in
    // coordinationEventsJson.
    property double swapEngagedMs: 0
    property var loopReceipt: null
    property int seenCompleted: 0

    function buildLoopReceipt() {
        var ctx = {
            status: "completed",
            lezAmount: swapBackend.lezAmount,
            ethAmountEth: swapBackend.ethAmount,
            ethHtlcAddress: swapBackend.ethHtlcAddress,
            lezProgramId: swapBackend.lezHtlcProgramId,
            lezTimelockMinutes: swapBackend.lezTimelockMinutes,
            ethTimelockMinutes: swapBackend.ethTimelockMinutes,
            startedMs: makerRoot.swapEngagedMs,
            finishedMs: Date.now()
        }
        try {
            var events = JSON.parse(swapBackend.coordinationEventsJson || "[]")
            for (var i = events.length - 1; i >= 0; i--) {
                var ev = events[i]
                if (ev && ev.eth_swap_id) {
                    ctx.hashlock = String(ev.hashlock || "")
                    ctx.ethSwapId = String(ev.eth_swap_id || "")
                    ctx.counterpartyEth = String(ev.taker_eth_address || "")
                    ctx.counterpartyLez = String(ev.taker_lez_account || "")
                    break
                }
            }
        } catch (e) {}
        return ctx
    }

    Connections {
        target: swapBackend

        function onAutoAcceptRunningChanged() {
            if (swapBackend.autoAcceptRunning) {
                // Fresh live session: the moment-of-completion card belongs
                // to the previous session — drop it.
                makerRoot.loopReceipt = null
                makerRoot.swapEngagedMs = 0
                makerRoot.seenCompleted = swapBackend.autoAcceptCompleted
            }
        }

        function onMakerCurrentStepChanged() {
            // First step past idle marks the moment a buyer engaged.
            if (swapBackend.autoAcceptRunning
                    && makerRoot.swapEngagedMs === 0
                    && swapBackend.makerCurrentStep !== ""
                    && swapBackend.makerCurrentStep !== "WaitingForEthLock"
                    && swapBackend.makerCurrentStep !== "AutoAcceptStarted") {
                makerRoot.swapEngagedMs = Date.now()
            }
        }

        function onAutoAcceptCompletedChanged() {
            if (swapBackend.autoAcceptCompleted <= makerRoot.seenCompleted) {
                makerRoot.seenCompleted = swapBackend.autoAcceptCompleted
                return
            }
            makerRoot.seenCompleted = swapBackend.autoAcceptCompleted
            makerRoot.loopReceipt = makerRoot.buildLoopReceipt()
            makerRoot.swapEngagedMs = 0
        }
    }

    // --- Your rate -------------------------------------------------------
    Card {
        SectionHeader {
            label: "Your rate"
            hairline: false
        }
        RowLayout {
            spacing: Theme.spacingSmall

            Text {
                text: swapBackend.lezAmount + " LEZ"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontLarge
                font.bold: true
                font.family: Theme.monoFont
            }
            Text {
                text: "for"
                color: Theme.textMuted
                font.pixelSize: Theme.fontSmall
            }
            Text {
                text: swapBackend.ethAmount + " ETH"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontLarge
                font.bold: true
                font.family: Theme.monoFont
            }
            Text {
                text: "per swap"
                color: Theme.textMuted
                font.pixelSize: Theme.fontSmall
            }
            Item { Layout.fillWidth: true }
        }
        Text {
            Layout.fillWidth: true
            text: {
                var bal = swapBackend.lezBalance
                var amt = Number(swapBackend.lezAmount)
                var n = (amt > 0) ? Math.floor(Number(bal) / amt) : 0
                return "Available: " + bal + " LEZ"
                       + (n > 0 ? "  (about " + n + " swaps at this rate)" : "")
            }
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
        }
        Text {
            Layout.fillWidth: true
            text: "Change your rate under Advanced settings in Setup."
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }
    }

    // --- Go live ---------------------------------------------------------
    Card {
        tone: swapBackend.autoAcceptRunning ? "active" : "neutral"

        SectionHeader {
            label: swapBackend.autoAcceptRunning ? "You are live" : "Go live"
            hairline: false
        }
        Text {
            Layout.fillWidth: true
            text: swapBackend.autoAcceptRunning
                  ? "Your offer is published and you're listening for buyers. "
                    + "Leave this running."
                  : "Publishes your rate as an offer buyers can take, and starts "
                    + "listening for them to lock their ETH."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        // Filled accent to go live; error-outlined ghost to stop.
        GhostButton {
            id: goLiveButton
            visible: swapBackend.autoAcceptRunning
            text: "Stop selling"
            enabled: !swapBackend.messagingLoading
            accented: false
            Layout.fillWidth: true
            Layout.preferredHeight: 42
            font.bold: true
            onClicked: swapBackend.stopAutoAccept()
        }
        PrimaryButton {
            visible: !swapBackend.autoAcceptRunning
            text: swapBackend.messagingRetrying
                  ? "Waiting for the swap network…"
                  : "Go live and publish my offer"
            enabled: !swapBackend.makerRunning && !swapBackend.takerRunning
                     && !swapBackend.messagingLoading && !swapBackend.messagingRetrying
            Layout.fillWidth: true
            Layout.preferredHeight: 42
            onClicked: swapBackend.startAutoAccept()
        }

        // Live status line, on the page rather than in a status bar.
        RowLayout {
            visible: swapBackend.autoAcceptRunning
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot {
                status: makerRoot.inSwap ? "working" : "live"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                text: makerRoot.inSwap
                      ? Copy.step(swapBackend.makerCurrentStep)
                      : "Listening for buyers…"
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
            }
        }
        Text {
            visible: swapBackend.autoAcceptRunning && makerRoot.cumulativeStats !== ""
            Layout.fillWidth: true
            text: makerRoot.cumulativeStats
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }
    }

    // The seller's own reassurance. Previously absent entirely: their LEZ sits
    // in escrow for minutes and nothing said it comes back.
    SafetyNote {
        visible: swapBackend.autoAcceptRunning
        text: Copy.lezAutoRefunds
    }

    // --- Swap in progress -------------------------------------------------
    Card {
        visible: makerRoot.inSwap

        SectionHeader {
            label: "Swap in progress"
            hairline: false
        }
        ProgressStepper {
            Layout.fillWidth: true
            steps: makerRoot.makerSteps
            currentStep: swapBackend.makerCurrentStep
            completedSteps: makerRoot.completedSteps
        }
    }

    // --- The sale that just completed -------------------------------------
    ReceiptCard {
        visible: makerRoot.loopReceipt !== null
        Layout.fillWidth: true
        role: "maker"
        context: makerRoot.loopReceipt
    }

    // --- Offline / nothing-yet state --------------------------------------
    Card {
        visible: !swapBackend.autoAcceptRunning

        EmptyState {
            Layout.fillWidth: true
            tone: Theme.toneWaiting
            title: makerRoot.cumulativeStats !== ""
                   ? "You're offline" : "Not selling yet"
            subtitle: makerRoot.cumulativeStats !== ""
                      ? makerRoot.cumulativeStats + ". Go live again whenever you like."
                      : "Go live to publish your rate. Buyers will see it on the market "
                        + "and can take it while you're running."
        }
    }

    // --- Completed sales ---------------------------------------------------
    Card {
        visible: swapBackend.swapHistory.length > 0

        SectionHeader {
            label: "This session"
            detail: "" + swapBackend.swapHistory.length
            hairline: false
        }

        Repeater {
            model: swapBackend.swapHistory
            delegate: ColumnLayout {
                id: entryCol
                required property var modelData

                Layout.fillWidth: true
                spacing: 4

                property var entry: {
                    try { return JSON.parse(modelData) }
                    catch (e) { return { status: "unknown" } }
                }

                Rectangle {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 1
                    Layout.topMargin: Theme.spacingSmall
                    color: Theme.border
                    opacity: 0.6
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: Theme.spacingSmall

                    StatusDot {
                        status: entryCol.entry.status === "completed" ? "live" : "problem"
                        size: 6
                        Layout.alignment: Qt.AlignVCenter
                    }
                    Text {
                        text: {
                            var e = entryCol.entry
                            if (e.status === "completed")
                                return "Sold " + e.lez_amount + " LEZ for " + e.eth_amount + " ETH"
                            if (e.status === "failed")
                                return "Didn't complete"
                            if (e.status === "insufficient_funds")
                                return "Not enough LEZ"
                            return e.status
                        }
                        color: entryCol.entry.status === "completed"
                               ? Theme.textPrimary : Theme.toneProblem
                        font.pixelSize: Theme.fontSmall
                        font.bold: true
                    }
                    Item { Layout.fillWidth: true }
                    Text {
                        text: Format.timeAgo(entryCol.entry.timestamp)
                        color: Theme.textMuted
                        font.pixelSize: Theme.fontCaption
                        font.family: Theme.monoFont
                    }
                }

                HexValue {
                    visible: entryCol.entry.status === "completed" && !!entryCol.entry.eth_tx
                    label: "ETH claim"
                    value: entryCol.entry.eth_tx || ""
                }
                HexValue {
                    visible: entryCol.entry.status === "completed" && !!entryCol.entry.lez_tx
                    label: "LEZ lock"
                    value: entryCol.entry.lez_tx || ""
                }

                Text {
                    visible: entryCol.entry.status === "insufficient_funds"
                    Layout.fillWidth: true
                    text: "You had " + (entryCol.entry.lez_balance || "?")
                          + " LEZ but the swap needed "
                          + (entryCol.entry.lez_required || "?") + " LEZ."
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontCaption
                    horizontalAlignment: Text.AlignLeft
                    wrapMode: Text.WordWrap
                }
                ColumnLayout {
                    visible: entryCol.entry.status === "failed" && !!entryCol.entry.error
                    Layout.fillWidth: true
                    spacing: 2
                    Text {
                        Layout.fillWidth: true
                        text: "Nothing was lost — anything locked comes back after its timer."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontCaption
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.WordWrap
                    }
                    Text {
                        Layout.fillWidth: true
                        text: entryCol.entry.error || ""
                        color: Theme.textMuted
                        font.pixelSize: Theme.fontCaption
                        font.family: Theme.monoFont
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.Wrap
                    }
                }
            }
        }
    }
}
