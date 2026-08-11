import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme
import SwapLinks

ScrollView {
    id: takerRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    // Each step carries a short `hint` — a calm expectation line the stepper
    // shows under the active step. The multi-minute waits (steps 4/5) are
    // NORMAL on the LEZ testnet, so the copy says so and reminds the user
    // their locked ETH auto-refunds if the maker never completes.
    property var takerSteps: [
        { name: "PreimageGenerated", label: "Generate Preimage",
          hint: "Generating your secret. Nothing is on-chain yet." },
        { name: "LockingEth",        label: "Lock ETH",
          hint: "Locking your ETH in the HTLC on Sepolia — usually under a minute." },
        { name: "EthLocked",         label: "ETH Locked",
          hint: "Your ETH is locked. If the maker never locks LEZ, it auto-refunds "
                + "after the timelock — a stalled swap cannot lose your funds." },
        { name: "WaitingForLezLock", label: "Wait for LEZ Lock",
          hint: "Waiting for the maker to detect your lock and lock LEZ. Typically "
                + "1–5 min — LEZ blocks can be a minute or more apart, so a wait "
                + "here is normal, not stuck. If the maker never locks, your ETH "
                + "auto-refunds after the timelock." },
        { name: "LezLockDetected",   label: "LEZ Lock Detected",
          hint: "Maker locked LEZ. Verifying the escrow terms match your swap." },
        { name: "VerifyingLezEscrow", label: "Verify LEZ Escrow",
          hint: "Checking the maker's LEZ escrow (amount, hashlock, timelock). "
                + "Usually seconds." },
        { name: "LezEscrowVerified", label: "LEZ Escrow Verified",
          hint: "Escrow verified. Claiming your LEZ next." },
        { name: "ClaimingLez",       label: "Claim LEZ",
          hint: "Claiming your LEZ by revealing the preimage — typically 1–5 min "
                + "for the LEZ chain to confirm." },
        { name: "LezClaimed",        label: "LEZ Claimed",
          hint: "" },
    ]

    // Steps where the taker's ETH is locked and the maker's completion is
    // still pending — the window where "am I stuck?" bites and where the
    // auto-refund reassurance + on-chain lock proof are worth surfacing.
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

    property var discoveredOffers: []
    property var pendingOffer: null
    property var acceptedOffer: null
    property bool swapCompleted: false

    // Receipt context, captured in QML at the run boundaries (session-only,
    // PR1): the accepted offer is snapshotted *before* it is cleared at
    // completion, and wall-clock stamps bracket the run.
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
        // immediate post-swap card has the same ETH lock proof and network
        // facts as History, instead of waiting for the user to change tabs.
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

    // Convert wei to ETH numeric string (for config fields, not display)
    function weiToEthValue(wei) {
        var n = Number(wei)
        if (isNaN(n) || n === 0) return "0"
        var eth = n / 1e18
        return eth.toString()
    }

    function weiToEth(wei) {
        var n = Number(wei)
        if (isNaN(n) || n === 0) return "0 ETH"
        var eth = n / 1e18
        if (eth >= 0.001) return eth.toFixed(6).replace(/\.?0+$/, '') + " ETH"
        // Show in Gwei for small amounts
        var gwei = n / 1e9
        if (gwei >= 1) return gwei.toFixed(4).replace(/\.?0+$/, '') + " Gwei"
        return wei + " wei"
    }

    function timeAgo(timestampMs) {
        if (!timestampMs) return ""
        var diff = Date.now() - timestampMs
        if (diff < 0) diff = 0
        var sec = Math.floor(diff / 1000)
        if (sec < 60) return sec + "s ago"
        var min = Math.floor(sec / 60)
        if (min < 60) return min + "m ago"
        var hr = Math.floor(min / 60)
        return hr + "h " + (min % 60) + "m ago"
    }

    // timelockSec is an absolute Unix timestamp in seconds
    function expiresIn(timelockSec) {
        if (!timelockSec) return ""
        var diff = timelockSec - Math.floor(Date.now() / 1000)
        if (diff <= 0) return "expired"
        var min = Math.floor(diff / 60)
        if (min < 60) return min + "m"
        var hr = Math.floor(min / 60)
        return hr + "h " + (min % 60) + "m"
    }

    Flickable {
        contentHeight: takerCol.implicitHeight + Theme.spacingXLarge * 2
        boundsBehavior: Flickable.StopAtBounds

        ColumnLayout {
            id: takerCol
            anchors {
                top: parent.top
                left: parent.left
                right: parent.right
                margins: Theme.spacingXLarge
            }
            spacing: Theme.spacingLarge

            Connections {
                target: swapBackend
                function onOffersFetched(offersJson) {
                    var obj = {}
                    try {
                        obj = JSON.parse(offersJson || "{}")
                    } catch (e) {
                        return
                    }
                    if (obj.offers && obj.offers.length > 0) {
                        // Merge new offers with existing (relay drains are destructive)
                        var merged = takerRoot.discoveredOffers.slice()
                        var seen = {}
                        for (var i = 0; i < merged.length; i++)
                            seen[merged[i].maker_eth_address + ":" + merged[i].lez_amount + ":" + merged[i].eth_amount] = true
                        for (var j = 0; j < obj.offers.length; j++) {
                            var o = obj.offers[j]
                            var key = o.maker_eth_address + ":" + o.lez_amount + ":" + o.eth_amount
                            if (!seen[key]) {
                                merged.push(o)
                                seen[key] = true
                            }
                        }
                        takerRoot.discoveredOffers = merged
                    }
                }
                function onTakerRunningChanged() {
                    if (swapBackend.takerRunning) {
                        // A run is starting (offer swap or ETH refund):
                        // stamp the wall clock and drop any stale snapshot
                        // so the receipt never mixes contexts.
                        takerRoot.swapStartedMs = Date.now()
                        takerRoot.swapFinishedMs = 0
                        takerRoot.completedOffer = null
                        return
                    }
                    takerRoot.swapFinishedMs = Date.now()
                    if (takerRoot.acceptedOffer !== null) {
                        // Snapshot the offer context *before* clearing it —
                        // the receipt renders amounts, counterparty and
                        // timelocks from it.
                        takerRoot.completedOffer = takerRoot.acceptedOffer
                        takerRoot.swapCompleted = true
                        takerRoot.acceptedOffer = null
                    }
                }
            }

            Text {
                text: "Buy LEZ"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            Text {
                visible: !swapBackend.takerRunning && !takerRoot.swapCompleted
                text: "Browse available offers and click one to start a swap."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontNormal
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // --- Discover Offers ---
            GhostButton {
                visible: !swapBackend.takerRunning && !takerRoot.swapCompleted
                text: swapBackend.messagingLoading
                      ? "Starting Delivery..."
                      : (swapBackend.offersLoading ? "Fetching..."
                      : (swapBackend.messagingRetrying ? "Waiting for Delivery..."
                      : (swapBackend.messagingConnected ? "Discover Offers" : "Waiting for Delivery...")
                      ))
                enabled: !swapBackend.offersLoading && !swapBackend.messagingLoading
                         && !swapBackend.messagingRetrying
                         && swapBackend.messagingConnected
                         && !swapBackend.running
                Layout.fillWidth: true
                Layout.preferredHeight: 42
                font.bold: true

                onClicked: {
                    swapBackend.fetchOffers()
                }
            }

            Text {
                visible: !swapBackend.takerRunning && !takerRoot.swapCompleted
                text: "Offers are advertisements — a swap completes only if the maker is live."
                color: Theme.textMuted
                font.pixelSize: Theme.fontCaption
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // Offer list \u2014 tape rows (offer-board idiom): hairline
            // separators, accent cursor bar on hover, monospace data.
            Repeater {
                model: !swapBackend.takerRunning && !takerRoot.swapCompleted ? discoveredOffers : []

                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: offerItemCol.implicitHeight + Theme.spacingNormal * 2
                    color: offerMouse.containsMouse
                           ? Qt.darker(Theme.surface, 1.05) : "transparent"

                    // Accent cursor bar on hover
                    Rectangle {
                        anchors.left: parent.left
                        width: 3
                        height: parent.height
                        color: Theme.accent
                        visible: offerMouse.containsMouse
                    }

                    // Hairline separator
                    Rectangle {
                        anchors.bottom: parent.bottom
                        width: parent.width
                        height: 1
                        color: Theme.border
                        opacity: 0.6
                    }

                    MouseArea {
                        id: offerMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        enabled: !swapBackend.running && takerRoot.pendingOffer === null
                        onClicked: {
                            takerRoot.pendingOffer = modelData
                        }
                    }

                    ColumnLayout {
                        id: offerItemCol
                        anchors {
                            fill: parent
                            margins: Theme.spacingNormal
                        }
                        spacing: 6

                        // Row 1: amounts + time
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacingNormal

                            Text {
                                text: modelData.lez_amount + " LEZ"
                                color: Theme.textPrimary
                                font.pixelSize: Theme.fontSmall
                                font.bold: true
                                font.family: Theme.monoFont
                            }
                            Text {
                                text: "\u21C4"
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontSmall
                            }
                            Text {
                                text: takerRoot.weiToEth(modelData.eth_amount)
                                color: Theme.textPrimary
                                font.pixelSize: Theme.fontSmall
                                font.bold: true
                                font.family: Theme.monoFont
                            }
                            Item { Layout.fillWidth: true }
                            Text {
                                text: takerRoot.timeAgo(modelData.timestamp_ms)
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontCaption
                                font.family: Theme.monoFont
                            }
                        }

                        // Row 2: maker address + timelocks
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacingNormal

                            Text {
                                text: "Maker " + modelData.maker_eth_address.substring(0, 10) + "\u2026"
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontDetail
                                font.family: Theme.monoFont
                            }
                            Item { Layout.fillWidth: true }
                            Text {
                                text: "LEZ " + takerRoot.expiresIn(modelData.lez_timelock)
                                      + " / ETH " + takerRoot.expiresIn(modelData.eth_timelock)
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontCaption
                                font.family: Theme.monoFont
                            }
                        }
                    }
                }
            }

            // Connecting hint — beacon empty state (offer-board idiom)
            EmptyState {
                visible: !swapBackend.messagingConnected && !swapBackend.takerRunning && !takerRoot.swapCompleted
                Layout.fillWidth: true
                Layout.topMargin: Theme.spacingLarge
                tone: Theme.warning
                title: "Connecting to the swap network…"
                subtitle: swapBackend.messagingRetrying
                      ? "Delivery is starting automatically. Offers will be received once infra is ready."
                      : "Delivery is starting automatically."
            }

            // No offers message — beacon empty state (offer-board idiom)
            EmptyState {
                visible: discoveredOffers.length === 0 && !swapBackend.offersLoading && !swapBackend.messagingLoading && !swapBackend.takerRunning && !takerRoot.swapCompleted && takerRoot.pendingOffer === null && swapBackend.messagingConnected
                Layout.fillWidth: true
                Layout.topMargin: Theme.spacingLarge
                tone: Theme.accent
                title: "No offers found yet"
                subtitle: "Click \"Discover Offers\" to search, or watch the Market tab — offers appear the moment makers publish them."
            }

            // --- Confirm Purchase Card ---
            Rectangle {
                visible: takerRoot.pendingOffer !== null && !swapBackend.takerRunning
                Layout.fillWidth: true
                implicitHeight: confirmCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.accent
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: confirmCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: 8

                    SectionHeader {
                        label: "Confirm purchase"
                        hairline: false
                    }
                    Text {
                        text: takerRoot.pendingOffer
                              ? "Buy " + takerRoot.pendingOffer.lez_amount + " LEZ for " + takerRoot.weiToEth(takerRoot.pendingOffer.eth_amount) + "?"
                              : ""
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontLarge
                        font.bold: true
                    }
                    Text {
                        text: takerRoot.pendingOffer
                              ? "from " + takerRoot.pendingOffer.maker_eth_address.substring(0, 6) + "…" + takerRoot.pendingOffer.maker_eth_address.substring(takerRoot.pendingOffer.maker_eth_address.length - 4)
                              : ""
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        font.family: Theme.monoFont
                    }
                    Text {
                        text: "Starting will lock ETH and wait for the maker listener to lock LEZ."
                        color: Theme.warning
                        font.pixelSize: Theme.fontSmall
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Theme.spacingNormal

                        PrimaryButton {
                            text: "Buy"
                            enabled: !swapBackend.running
                            Layout.fillWidth: true
                            Layout.preferredHeight: 40

                            onClicked: {
                                var offer = takerRoot.pendingOffer
                                takerRoot.acceptedOffer = offer
                                takerRoot.pendingOffer = null
                                swapBackend.acceptOfferAndStartTaker(offer)
                            }
                        }

                        GhostButton {
                            text: "Cancel"
                            accented: false
                            enabled: !swapBackend.running
                            Layout.fillWidth: true
                            Layout.preferredHeight: 40

                            onClicked: {
                                takerRoot.pendingOffer = null
                            }
                        }
                    }
                }
            }

            // --- Active Swap: Accepted Offer + Progress ---
            Rectangle {
                visible: swapBackend.takerRunning && takerRoot.acceptedOffer !== null
                Layout.fillWidth: true
                implicitHeight: activeCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.accent
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: activeCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: 6

                    SectionHeader {
                        label: "Active swap"
                        hairline: false
                    }
                    Text {
                        text: takerRoot.acceptedOffer
                              ? "Buying " + takerRoot.acceptedOffer.lez_amount + " LEZ for " + takerRoot.weiToEth(takerRoot.acceptedOffer.eth_amount)
                              : ""
                        color: Theme.accent
                        font.pixelSize: Theme.fontLarge
                        font.bold: true
                    }
                    Text {
                        text: takerRoot.acceptedOffer
                              ? "from " + takerRoot.acceptedOffer.maker_eth_address.substring(0, 6) + "…" + takerRoot.acceptedOffer.maker_eth_address.substring(takerRoot.acceptedOffer.maker_eth_address.length - 4)
                              : ""
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        font.family: Theme.monoFont
                    }
                }
            }

            // Progress (only during active swap)
            Rectangle {
                visible: swapBackend.takerRunning
                Layout.fillWidth: true
                implicitHeight: takerProgressCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: takerProgressCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: Theme.spacingSmall

                    SectionHeader {
                        label: "Swap in progress"
                        hairline: false
                    }
                    ProgressStepper {
                        id: takerStepper
                        Layout.fillWidth: true
                        steps: takerSteps
                        currentStep: swapBackend.takerCurrentStep
                        completedSteps: takerRoot.completedSteps
                    }

                    // Auto-refund reassurance, with a live countdown to the
                    // ETH timelock. The accepted offer carries the absolute
                    // eth_timelock the taker adopts at lock time, so this is
                    // the real deadline. Re-evaluated every second by reading
                    // the stepper's ticking elapsed counter as a dependency —
                    // no extra Timer (which, as a direct ScrollView child,
                    // would break the layout, #113).
                    Text {
                        visible: takerRoot.acceptedOffer !== null
                                 && takerRoot.ethLockedSteps.indexOf(swapBackend.takerCurrentStep) >= 0
                        Layout.fillWidth: true
                        Layout.topMargin: Theme.spacingSmall
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontCaption
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.WordWrap
                        text: {
                            var _tick = takerStepper.activeElapsedSeconds // 1s refresh dep
                            var remaining = takerRoot.acceptedOffer
                                ? takerRoot.expiresIn(takerRoot.acceptedOffer.eth_timelock)
                                : ""
                            if (remaining === "" || remaining === "expired")
                                return "Your ETH is safe: it auto-refunds once the ETH "
                                       + "timelock passes if the maker never completes — "
                                       + "a stuck swap cannot lose funds."
                            return "Your ETH is safe: if this stalls, it auto-refunds in ~"
                                   + remaining + " (ETH timelock) — a stuck swap cannot "
                                   + "lose funds."
                        }
                    }

                    // On-chain lock proof, surfaced mid-swap so a user can
                    // check the tx on Sepolia themselves instead of wondering
                    // whether their lock landed. Copy-only + explorer link
                    // (the receipt-card idiom); only shown once the backend
                    // has the hash from the EthLocked progress event.
                    TrustRow {
                        visible: swapBackend.takerEthLockTx !== ""
                        Layout.fillWidth: true
                        Layout.topMargin: Theme.spacingSmall
                        label: "Your ETH lock tx — verify it on-chain"
                        value: swapBackend.takerEthLockTx
                        link: Links.ethTx(swapBackend.takerEthLockTx,
                                          swapBackend.takerEthChainId)
                    }
                }
            }

            // Receipt — evidence surface for the just-completed swap
            ReceiptCard {
                role: "taker"
                resultJson: swapBackend.takerResultJson
                context: takerRoot.receiptContext
            }

            // --- Browse More Offers (post-swap) ---
            GhostButton {
                visible: takerRoot.swapCompleted && !swapBackend.takerRunning
                text: "Browse More Offers"
                Layout.fillWidth: true
                Layout.preferredHeight: 42
                font.bold: true

                onClicked: {
                    takerRoot.swapCompleted = false
                    takerRoot.pendingOffer = null
                    takerRoot.acceptedOffer = null
                    takerRoot.discoveredOffers = []
                }
            }
        }
    }
}
