import QtQuick
import QtQuick.Layouts
import SwapTheme

// Receipt card — the full evidence surface for the just-completed swap.
// Composes the board's trust vocabulary (TrustRow, section dividers,
// GhostButton) into the offer detail pane's sibling: hero amounts + rate,
// both chain references labeled per role, hashlock, preimage with a
// reveal affordance, counterparty and contract identity, timelocks, the
// completion wall-clock time, and a copyable JSON receipt.
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

    property bool preimageRevealed: false
    property bool copied: false

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

    // JSON receipt (schema swap-receipt/1, dossier §3b): strings stay
    // strings, unknown fields are null.
    function receiptJson() {
        var r = {
            schema: "swap-receipt/1",
            role: role,
            status: outcome,
            hashlock: hashlock || null,
            preimage: preimage || null,
            lez_amount: lezAmount || null,
            eth_amount_wei: ethAmountWei || null,
            eth_amount: ethAmountEth || null,
            eth: {
                swap_id: ethSwapId || null,
                claim_tx: ethClaimTx || null,
                refund_tx: ethRefundTx || null,
                htlc_address: ethHtlcAddress || null,
                counterparty: counterpartyEth || null
            },
            lez: {
                lock_tx: lezLockTx || null,
                claim_tx: lezClaimTx || null,
                refund_tx: lezRefundTx || null,
                program_id: lezProgramId || null,
                counterparty: counterpartyLez || null
            },
            timelocks: {
                lez_unix: lezTimelockUnix > 0 ? lezTimelockUnix : null,
                eth_unix: ethTimelockUnix > 0 ? ethTimelockUnix : null,
                lez_minutes: lezTimelockMinutes || null,
                eth_minutes: ethTimelockMinutes || null
            },
            started_ms: startedMs > 0 ? startedMs : null,
            finished_ms: finishedMs > 0 ? finishedMs : null
        }
        if (isError && parsed)
            r.error = parsed.error
        return JSON.stringify(r, null, 2)
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
        onTriggered: card.copied = false
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

        // Chain references, labeled per role (completed swaps)
        TrustRow {
            visible: card.isCompleted && card.role === "taker"
            label: "ETH swap ID (lock)"
            value: card.ethSwapId
        }
        TrustRow {
            visible: card.isCompleted && card.role === "taker"
            label: "LEZ claim tx"
            value: card.lezClaimTx
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
        }
        TrustRow {
            visible: card.isCompleted && card.role === "maker"
            label: "ETH claim tx"
            value: card.ethClaimTx
        }

        // Refund evidence (refunded swaps)
        TrustRow {
            visible: card.isRefunded
            label: "ETH refund tx"
            value: card.ethRefundTx !== "" ? card.ethRefundTx : "n/a"
        }
        TrustRow {
            visible: card.isRefunded
            label: "LEZ refund tx"
            value: card.lezRefundTx !== "" ? card.lezRefundTx : "n/a"
        }

        // Counterparty identity
        TrustRow {
            visible: !card.isError && card.counterpartyEth !== ""
            label: card.counterpartyName + " ETH address"
            value: card.counterpartyEth
        }
        TrustRow {
            visible: !card.isError && card.counterpartyLez !== ""
            label: card.counterpartyName + " LEZ account"
            value: card.counterpartyLez
        }

        // Contract identity
        TrustRow {
            visible: !card.isError && card.ethHtlcAddress !== ""
            label: "ETH HTLC contract"
            value: card.ethHtlcAddress
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
            visible: !card.isError
            Layout.fillWidth: true
            Layout.topMargin: Theme.spacingSmall
            spacing: Theme.spacingNormal

            GhostButton {
                text: card.copied ? "Copied" : "Copy JSON receipt"
                accented: false
                Layout.preferredHeight: 32
                font.pixelSize: Theme.fontCaption

                onClicked: {
                    clipboardHelper.text = card.receiptJson()
                    clipboardHelper.selectAll()
                    clipboardHelper.copy()
                    clipboardHelper.text = ""
                    card.copied = true
                    copiedReset.restart()
                }
            }
            Text {
                Layout.fillWidth: true
                text: "Session receipt — not persisted. Copy the JSON to keep it."
                color: Theme.textMuted
                font.pixelSize: Theme.fontCaption
                wrapMode: Text.Wrap
            }
        }
    }
}
