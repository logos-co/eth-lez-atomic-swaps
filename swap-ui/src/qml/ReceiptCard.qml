import QtQuick
import QtQuick.Layouts
import SwapTheme
import SwapLinks

// Receipt card — the full evidence surface for the just-completed swap.
// Composes the board's trust vocabulary (TrustRow, section dividers,
// GhostButton) into the offer detail pane's sibling: hero amounts + rate,
// both chain references labeled per role, hashlock, preimage with a
// reveal affordance, counterparty and contract identity, timelocks, the
// completion wall-clock time, and secret-safe public evidence.
//
// Inputs:
//  - role: "taker" | "maker". The backend's `eth_tx` result field is
//    role-dependent (src/swap/types.rs): the maker's ETH *claim tx hash*
//    but the taker's ETH lock *swap ID* — not a tx hash. The receipt
//    labels it accordingly instead of calling both "ETH Tx".
//  - resultJson: authoritative outcome JSON (taker/makerResultJson). May
//    be empty for maker auto-accept completions, which carry no per-swap
//    result JSON until the backend forwards outcomes (later PR).
//  - context: session snapshot assembled in QML by the owning view
//    (amounts, counterparty, contracts, timelocks, wall-clock stamps).
//    Amount-ish values stay strings end-to-end; Number() is used for
//    display math only (rate, wei→ETH) — the OfferBoard convention.
Rectangle {
    id: card

    required property string role
    property string resultJson: ""
    property var context: null
    // The moment-of-completion card points at the History tab archive; the
    // History tab's own expanded card hides the (self-referential) hint.
    property bool showHistoryHint: true

    property bool preimageRevealed: false
    property string copiedKind: ""

    readonly property string feedbackUrl: "https://github.com/logos-co/eth-lez-atomic-swaps/issues/new?template=trial-feedback.yml"

    readonly property var parsed: {
        if (!resultJson) return null
        try { return JSON.parse(resultJson) }
        catch (e) { return { error: resultJson } }
    }
    readonly property var ctx: context || ({})

    readonly property bool isError: parsed !== null && parsed.error !== undefined
    readonly property string outcome: {
        if (isError) return "error"
        if (parsed && parsed.status) return parsed.status
        return ctx.status || ""
    }
    readonly property bool isCompleted: outcome === "completed"
    readonly property bool isRefunded: outcome === "refunded"

    readonly property color tone: isError ? Theme.error
                                          : isCompleted ? Theme.success
                                                        : Theme.warning

    // --- Evidence (result JSON first, session context as fallback) -----
    readonly property string hashlock: (parsed && parsed.hashlock ? parsed.hashlock : "")
                                       || ctx.hashlock || ""
    readonly property string preimage: parsed && parsed.preimage ? parsed.preimage : ""
    // eth_tx per role (the mislabel fix): taker → ETH lock swap ID,
    // maker → ETH claim tx hash.
    readonly property string ethSwapId: role === "taker"
        ? ((parsed && parsed.eth_tx ? parsed.eth_tx : "") || ctx.ethSwapId || "")
        : (ctx.ethSwapId || "")
    readonly property string ethClaimTx: role === "maker" && parsed && parsed.eth_tx
        ? parsed.eth_tx : ""
    // lez_tx per role: maker → LEZ lock tx, taker → LEZ claim tx.
    readonly property string lezLockTx: role === "maker" && parsed && parsed.lez_tx
        ? parsed.lez_tx : ""
    readonly property string lezClaimTx: role === "taker" && parsed && parsed.lez_tx
        ? parsed.lez_tx : ""
    readonly property string ethRefundTx: parsed && parsed.eth_refund_tx
        ? parsed.eth_refund_tx : ""
    readonly property string lezRefundTx: parsed && parsed.lez_refund_tx
        ? parsed.lez_refund_tx : ""
    // eth.lock_tx / network are not part of the legacy result-JSON schema
    // (src/swap/types.rs outcome_to_json) — they only exist on the
    // persisted receipt / session context, so ctx is their only source.
    readonly property string ethLockTx: ctx.ethLockTx || ""
    readonly property var networkCtx: ctx.network || ({})
    readonly property string lezSequencer: networkCtx.lezSequencer || ""
    readonly property double ethChainId: Number(networkCtx.ethChainId || 0)
    readonly property var iterationValue: ctx.iteration !== undefined && ctx.iteration !== null
        ? ctx.iteration : null
    // The LEZ explorer only indexes the public testnet — gate lezTx/
    // lezAccount links on the run's actual sequencer, not a config claim.
    readonly property bool lezExplorerAvailable: Links.isLezTestnet(lezSequencer)

    // --- Session context ------------------------------------------------
    readonly property string lezAmount: ctx.lezAmount || ""
    readonly property string ethAmountWei: ctx.ethAmountWei || ""
    readonly property string ethAmountEth: ctx.ethAmountEth || ""
    readonly property bool hasAmounts: lezAmount !== ""
        && (ethAmountWei !== "" || ethAmountEth !== "")
    readonly property string counterpartyEth: ctx.counterpartyEth || ""
    readonly property string counterpartyLez: ctx.counterpartyLez || ""
    readonly property string counterpartyName: role === "taker" ? "Maker" : "Taker"
    readonly property string ethHtlcAddress: ctx.ethHtlcAddress || ""
    readonly property string lezProgramId: ctx.lezProgramId || ""
    readonly property double lezTimelockUnix: Number(ctx.lezTimelockUnix || 0)
    readonly property double ethTimelockUnix: Number(ctx.ethTimelockUnix || 0)
    readonly property string lezTimelockMinutes: ctx.lezTimelockMinutes || ""
    readonly property string ethTimelockMinutes: ctx.ethTimelockMinutes || ""
    readonly property bool hasTimelocks: lezTimelockUnix > 0 || ethTimelockUnix > 0
        || lezTimelockMinutes !== "" || ethTimelockMinutes !== ""
    readonly property double startedMs: Number(ctx.startedMs || 0)
    readonly property double finishedMs: Number(ctx.finishedMs || 0)

    // --- Display helpers (Number() for display only) --------------------
    function ethDisplay() {
        if (ethAmountWei !== "") {
            var n = Number(ethAmountWei)
            if (isNaN(n) || n === 0) return "0 ETH"
            var eth = n / 1e18
            if (eth >= 0.001) return eth.toFixed(6).replace(/\.?0+$/, '') + " ETH"
            var gwei = n / 1e9
            if (gwei >= 1) return gwei.toFixed(4).replace(/\.?0+$/, '') + " Gwei"
            return ethAmountWei + " wei"
        }
        if (ethAmountEth !== "") return ethAmountEth + " ETH"
        return ""
    }

    function rate() {
        var lez = Number(lezAmount)
        var eth = ethAmountWei !== "" ? Number(ethAmountWei) / 1e18
                                      : Number(ethAmountEth)
        if (!(lez > 0) || !(eth > 0)) return 0
        return lez / eth
    }

    function fmtRate(r) {
        if (!(r > 0)) return "—"
        if (r >= 100) return r.toFixed(0)
        if (r >= 1) return r.toFixed(2)
        return r.toFixed(6)
    }

    function fmtClock(ms) {
        return Qt.formatDateTime(new Date(ms), "hh:mm:ss")
    }

    function fmtDuration(ms) {
        var sec = Math.max(0, Math.round(ms / 1000))
        if (sec < 60) return sec + "s"
        var min = Math.floor(sec / 60)
        if (min < 60) return min + "m " + (sec % 60) + "s"
        return Math.floor(min / 60) + "h " + (min % 60) + "m"
    }

    function fmtTimelock(unixSec, minutes) {
        var parts = []
        if (unixSec > 0)
            parts.push(Qt.formatDateTime(new Date(unixSec * 1000), "MMM d, hh:mm:ss"))
        if (minutes !== "")
            parts.push(minutes + "m window")
        return parts.length > 0 ? parts.join(" · ") : "—"
    }

    // Public feedback evidence is deliberately narrower than the archived
    // receipt. Never copy the preimage, hashlock, RPC URL, wallet addresses,
    // raw backend error, or config-derived identities into a public report.
    function safeEvidenceJson() {
        var r = {
            schema: "trial-evidence/1",
            role: role,
            status: outcome,
            ethereum: {
                chain_id: ethChainId > 0 ? ethChainId : null,
                lock_tx: ethLockTx || null,
                lock_url: Links.ethTx(ethLockTx, ethChainId) || null,
                claim_tx: ethClaimTx || null,
                claim_url: Links.ethTx(ethClaimTx, ethChainId) || null,
                refund_tx: ethRefundTx || null,
                refund_url: Links.ethTx(ethRefundTx, ethChainId) || null
            },
            lez: {
                lock_tx: lezLockTx || null,
                lock_url: lezExplorerAvailable
                    ? (Links.lezTx(lezLockTx) || null) : null,
                claim_tx: lezClaimTx || null,
                claim_url: lezExplorerAvailable
                    ? (Links.lezTx(lezClaimTx) || null) : null,
                refund_tx: lezRefundTx || null,
                refund_url: lezExplorerAvailable
                    ? (Links.lezTx(lezRefundTx) || null) : null
            },
            started_ms: startedMs > 0 ? startedMs : null,
            finished_ms: finishedMs > 0 ? finishedMs : null
        }
        if (iterationValue !== null)
            r.iteration = iterationValue
        return JSON.stringify(r, null, 2)
    }

    function copyText(value, kind) {
        clipboardHelper.text = value
        clipboardHelper.selectAll()
        clipboardHelper.copy()
        clipboardHelper.text = ""
        copiedKind = kind
        copiedReset.restart()
    }

    visible: outcome !== ""
    Layout.fillWidth: true
    implicitHeight: receiptCol.implicitHeight + Theme.spacingNormal * 2
    radius: Theme.radiusNormal
    color: Theme.surface
    border.color: tone
    border.width: 1

    // Pure-QML clipboard: an invisible TextEdit whose selection is copied.
    TextEdit {
        id: clipboardHelper
        visible: false
    }

    Timer {
        id: copiedReset
        interval: 1600
        onTriggered: card.copiedKind = ""
    }

    ColumnLayout {
        id: receiptCol
        anchors {
            fill: parent
            margins: Theme.spacingNormal
        }
        spacing: Theme.spacingSmall

        // Header — tone dot + uppercase micro-label; wall-clock + duration
        RowLayout {
            Layout.fillWidth: true
            spacing: 6

            Rectangle {
                width: 6; height: 6; radius: 3
                color: card.tone
            }
            Text {
                text: card.isError ? "ERROR"
                                   : card.isCompleted ? "SWAP COMPLETED"
                                                      : card.isRefunded ? "SWAP REFUNDED"
                                                                        : "RESULT"
                color: card.tone
                font.pixelSize: Theme.fontCaption
                font.bold: true
                font.letterSpacing: 1
            }
            Item { Layout.fillWidth: true }
            Text {
                visible: card.finishedMs > 0
                text: card.fmtClock(card.finishedMs)
                      + (card.startedMs > 0 && card.finishedMs >= card.startedMs
                         ? " · " + card.fmtDuration(card.finishedMs - card.startedMs)
                         : "")
                color: Theme.textMuted
                font.pixelSize: Theme.fontCaption
                font.family: Theme.monoFont
            }
        }

        // Error message — keeps the current error-card presentation
        Text {
            visible: card.isError
            text: card.parsed ? (card.parsed.error || "") : ""
            color: Theme.textPrimary
            font.pixelSize: Theme.fontSmall
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }

        // Hero — role-phrased amounts + rate (offer detail pane idiom)
        Text {
            visible: !card.isError && card.hasAmounts
            text: card.isCompleted
                  ? (card.role === "taker" ? "Bought " : "Sold ") + card.lezAmount + " LEZ"
                  : "Swap of " + card.lezAmount + " LEZ"
            color: Theme.textPrimary
            font.pixelSize: Theme.fontTitle
            font.bold: true
        }
        Text {
            visible: !card.isError && card.hasAmounts
            text: "for " + card.ethDisplay()
            color: Theme.textSecondary
            font.pixelSize: Theme.fontLarge
        }
        Text {
            visible: !card.isError && card.hasAmounts && card.rate() > 0
            text: "1 ETH ≈ " + card.fmtRate(card.rate()) + " LEZ"
            color: Theme.textMuted
            font.pixelSize: Theme.fontSmall
            font.family: Theme.monoFont
        }

        Rectangle {
            visible: !card.isError
            Layout.fillWidth: true
            Layout.topMargin: Theme.spacingSmall
            Layout.bottomMargin: Theme.spacingSmall
            height: 1
            color: Theme.border
        }

        // --- Evidence stack --------------------------------------------
        TrustRow {
            visible: !card.isError
            label: "Hashlock"
            value: card.hashlock
        }

        // Preimage with reveal affordance. On a completed swap the
        // preimage is already public (revealed on-chain by the claim).
        ColumnLayout {
            visible: card.preimage !== ""
            spacing: 1
            Layout.fillWidth: true

            RowLayout {
                Layout.fillWidth: true
                Text {
                    text: "Preimage"
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontMicro
                    font.letterSpacing: 0.5
                }
                Item { Layout.fillWidth: true }
                Text {
                    text: card.preimageRevealed ? "Hide" : "Reveal"
                    color: Theme.accent
                    font.pixelSize: Theme.fontMicro
                    font.bold: true

                    MouseArea {
                        anchors.fill: parent
                        anchors.margins: -4
                        cursorShape: Qt.PointingHandCursor
                        onClicked: card.preimageRevealed = !card.preimageRevealed
                    }
                }
            }
            Text {
                Layout.fillWidth: true
                text: card.preimageRevealed
                      ? card.preimage
                      : "•••••••• — revealed on-chain at claim"
                color: Theme.textSecondary
                font.pixelSize: Theme.fontCaption
                font.family: Theme.monoFont
                elide: Text.ElideMiddle
            }
        }

        // Chain references, labeled per role (completed swaps). The ETH
        // swap ID stays copy-only everywhere — it's a contract-internal
        // bytes32 key, not a tx hash, and linking it to /tx/ would be a
        // mislabel (the reason ReceiptCard disambiguates eth_tx per role
        // in the first place).
        TrustRow {
            visible: card.isCompleted && card.ethLockTx !== ""
            label: "ETH lock tx"
            value: card.ethLockTx
            link: Links.ethTx(card.ethLockTx, card.ethChainId)
        }
        TrustRow {
            visible: card.isCompleted && card.role === "taker"
            label: "ETH swap ID (lock)"
            value: card.ethSwapId
        }
        TrustRow {
            visible: card.isCompleted && card.role === "taker"
            label: "LEZ claim tx"
            value: card.lezClaimTx
            link: card.lezExplorerAvailable ? Links.lezTx(card.lezClaimTx) : ""
        }
        TrustRow {
            visible: card.isCompleted && card.role === "maker" && card.ethSwapId !== ""
            label: "ETH swap ID (lock)"
            value: card.ethSwapId
        }
        TrustRow {
            visible: card.isCompleted && card.role === "maker"
            label: "LEZ lock tx"
            value: card.lezLockTx
            link: card.lezExplorerAvailable ? Links.lezTx(card.lezLockTx) : ""
        }
        TrustRow {
            visible: card.isCompleted && card.role === "maker"
            label: "ETH claim tx"
            value: card.ethClaimTx
            link: Links.ethTx(card.ethClaimTx, card.ethChainId)
        }

        // Refund evidence (refunded swaps)
        TrustRow {
            visible: card.isRefunded
            label: "ETH refund tx"
            value: card.ethRefundTx !== "" ? card.ethRefundTx : "n/a"
            link: Links.ethTx(card.ethRefundTx, card.ethChainId)
        }
        TrustRow {
            visible: card.isRefunded
            label: "LEZ refund tx"
            value: card.lezRefundTx !== "" ? card.lezRefundTx : "n/a"
            link: card.lezExplorerAvailable ? Links.lezTx(card.lezRefundTx) : ""
        }

        // Counterparty identity
        TrustRow {
            visible: !card.isError && card.counterpartyEth !== ""
            label: card.counterpartyName + " ETH address"
            value: card.counterpartyEth
            link: Links.ethAddress(card.counterpartyEth, card.ethChainId)
        }
        TrustRow {
            visible: !card.isError && card.counterpartyLez !== ""
            label: card.counterpartyName + " LEZ account"
            value: card.counterpartyLez
            link: card.lezExplorerAvailable ? Links.lezAccount(card.counterpartyLez) : ""
        }

        // Contract identity
        TrustRow {
            visible: !card.isError && card.ethHtlcAddress !== ""
            label: "ETH HTLC contract"
            value: card.ethHtlcAddress
            link: Links.ethAddress(card.ethHtlcAddress, card.ethChainId)
        }
        TrustRow {
            visible: !card.isError && card.lezProgramId !== ""
            label: "LEZ HTLC program"
            value: card.lezProgramId
        }

        // --- Timelocks --------------------------------------------------
        Rectangle {
            visible: !card.isError && card.hasTimelocks
            Layout.fillWidth: true
            Layout.topMargin: Theme.spacingSmall
            Layout.bottomMargin: Theme.spacingSmall
            height: 1
            color: Theme.border
        }
        RowLayout {
            visible: !card.isError && card.hasTimelocks
            Layout.fillWidth: true
            Text {
                text: "LEZ timelock"
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }
            Item { Layout.fillWidth: true }
            Text {
                text: card.fmtTimelock(card.lezTimelockUnix, card.lezTimelockMinutes)
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                font.family: Theme.monoFont
            }
        }
        RowLayout {
            visible: !card.isError && card.hasTimelocks
            Layout.fillWidth: true
            Text {
                text: "ETH timelock"
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }
            Item { Layout.fillWidth: true }
            Text {
                text: card.fmtTimelock(card.ethTimelockUnix, card.ethTimelockMinutes)
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                font.family: Theme.monoFont
            }
        }

        // --- Actions ----------------------------------------------------
        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: Theme.spacingSmall
            spacing: Theme.spacingNormal

            GhostButton {
                text: card.copiedKind === "evidence" ? "Safe evidence copied"
                                                       : "Copy safe evidence"
                accented: false
                Layout.preferredHeight: 32
                font.pixelSize: Theme.fontCaption

                onClicked: card.copyText(card.safeEvidenceJson(), "evidence")
            }
            GhostButton {
                text: card.copiedKind === "feedback" ? "Feedback link copied"
                                                       : "Copy feedback link"
                Layout.preferredHeight: 32
                font.pixelSize: Theme.fontCaption

                // Basecamp intentionally blocks module-owned external URLs.
                // Copy the stable HTTPS form URL until the host exposes an
                // approved, user-confirmed external-navigation capability.
                onClicked: card.copyText(card.feedbackUrl, "feedback")
            }
            Item { Layout.fillWidth: true }
        }
        Text {
            Layout.fillWidth: true
            text: card.copiedKind === "feedback"
                  ? "Feedback link copied. Paste it in your browser, then add the safe evidence."
                  : (card.showHistoryHint && !card.isError
                     ? "Saved to swap history. Basecamp keeps external links sandboxed; copy a URL and paste it in your browser."
                     : "Basecamp keeps external links sandboxed; copy the feedback link and paste it in your browser.")
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            wrapMode: Text.Wrap
        }
    }
}
