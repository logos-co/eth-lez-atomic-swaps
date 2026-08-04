import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme

// Swap history — the durable archive of every completed / refunded /
// failed swap on this profile, fed by the plugin's per-profile JSONL
// journal through the receiptsJson PROP (swap-receipt/1 objects, newest
// first). Rows use the offer tape idiom (hairline separators, monospace
// data, status dot); clicking a row expands the full ReceiptCard.
ScrollView {
    id: historyRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    readonly property var receipts: {
        try {
            var arr = JSON.parse(swapBackend.receiptsJson || "[]")
            return Array.isArray(arr) ? arr : []
        } catch (e) {
            return []
        }
    }
    property int expandedIndex: -1
    property bool clearArmed: false

    function weiToEth(wei) {
        var n = Number(wei)
        if (isNaN(n) || n === 0) return "0 ETH"
        var eth = n / 1e18
        if (eth >= 0.001) return eth.toFixed(6).replace(/\.?0+$/, '') + " ETH"
        var gwei = n / 1e9
        if (gwei >= 1) return gwei.toFixed(4).replace(/\.?0+$/, '') + " Gwei"
        return wei + " wei"
    }

    function ethDisplayOf(r) {
        if (r.eth_amount_wei) return weiToEth(r.eth_amount_wei)
        if (r.eth_amount) return r.eth_amount + " ETH"
        return ""
    }

    function heroOf(r) {
        var lez = r.lez_amount ? r.lez_amount + " LEZ" : ""
        var eth = ethDisplayOf(r)
        var pair = lez !== "" && eth !== "" ? lez + " ⇄ " + eth : (lez || eth)
        if (r.status === "completed") {
            var verb = r.role === "taker" ? "Bought " : "Sold "
            return pair !== "" ? verb + pair : verb.trim()
        }
        if (r.status === "refunded") return pair !== "" ? "Refunded — " + pair : "Refunded"
        if (r.status === "failed") return pair !== "" ? "Failed — " + pair : "Failed"
        return (r.status || "unknown") + (pair !== "" ? " — " + pair : "")
    }

    function toneOf(status) {
        if (status === "completed") return Theme.success
        if (status === "refunded") return Theme.warning
        return Theme.error
    }

    function stampOf(r) {
        var ms = Number(r.finished_ms || r.started_ms || 0)
        if (!(ms > 0)) return ""
        return Qt.formatDateTime(new Date(ms), "MMM d, hh:mm:ss")
    }

    // Adapter: synthesize the role-dependent result JSON ReceiptCard
    // expects, resolving the journal's already-disambiguated eth/lez
    // sections back into outcome_to_json field semantics (eth_tx is the
    // maker's claim tx but the taker's swap id; lez_tx is maker lock /
    // taker claim).
    function resultJsonOf(r) {
        var eth = r.eth || {}
        var lez = r.lez || {}
        var out = { status: r.status || "" }
        if (r.hashlock) out.hashlock = r.hashlock
        if (r.preimage) out.preimage = r.preimage
        var ethTx = r.role === "maker" ? eth.claim_tx : eth.swap_id
        if (ethTx) out.eth_tx = ethTx
        var lezTx = r.role === "maker" ? lez.lock_tx : lez.claim_tx
        if (lezTx) out.lez_tx = lezTx
        if (eth.refund_tx) out.eth_refund_tx = eth.refund_tx
        if (lez.refund_tx) out.lez_refund_tx = lez.refund_tx
        if (r.error) out.error = r.error
        return JSON.stringify(out)
    }

    function contextOf(r) {
        var eth = r.eth || {}
        var lez = r.lez || {}
        var tl = r.timelocks || {}
        var net = r.network || {}
        return {
            status: r.status || "",
            hashlock: r.hashlock || "",
            ethSwapId: eth.swap_id || "",
            ethLockTx: eth.lock_tx || "",
            lezAmount: r.lez_amount || "",
            ethAmountWei: r.eth_amount_wei || "",
            ethAmountEth: r.eth_amount || "",
            counterpartyEth: eth.counterparty || "",
            counterpartyLez: lez.counterparty || "",
            ethHtlcAddress: eth.htlc_address || "",
            lezProgramId: lez.program_id || "",
            lezTimelockUnix: Number(tl.lez_unix || 0),
            ethTimelockUnix: Number(tl.eth_unix || 0),
            lezTimelockMinutes: tl.lez_minutes || "",
            ethTimelockMinutes: tl.eth_minutes || "",
            startedMs: Number(r.started_ms || 0),
            finishedMs: Number(r.finished_ms || 0),
            // network is a fact about the run (which chain/sequencer the
            // evidence was produced against), not evidence itself — kept
            // as its own sub-object so ReceiptCard can derive explorer
            // links (SwapLinks) without guessing at the chain.
            network: {
                lezSequencer: net.lez_sequencer || "",
                ethRpc: net.eth_rpc || "",
                ethChainId: Number(net.eth_chain_id || 0)
            },
            iteration: r.iteration !== undefined && r.iteration !== null
                ? r.iteration : null
        }
    }

    // QML lesson: the Flickable must be the ScrollView's first object
    // child — Qt adopts the first appended object as the view, so nothing
    // (not even a non-Item like a Timer or Connections) may precede it.
    Flickable {
        contentHeight: historyCol.implicitHeight + Theme.spacingXLarge * 2
        boundsBehavior: Flickable.StopAtBounds

        ColumnLayout {
            id: historyCol
            anchors {
                top: parent.top
                left: parent.left
                right: parent.right
                margins: Theme.spacingXLarge
            }
            spacing: Theme.spacingLarge

            Timer {
                id: clearDisarm
                interval: 3000
                onTriggered: historyRoot.clearArmed = false
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: Theme.spacingNormal

                Text {
                    text: "Swap History"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontTitle
                    font.bold: true
                }
                StatusChip {
                    visible: historyRoot.receipts.length > 0
                    text: historyRoot.receipts.length + " swap"
                          + (historyRoot.receipts.length !== 1 ? "s" : "")
                    tone: Theme.textSecondary
                    showDot: false
                }
                Item { Layout.fillWidth: true }
                GhostButton {
                    text: "Refresh"
                    accented: false
                    Layout.preferredHeight: 32
                    font.pixelSize: Theme.fontCaption
                    onClicked: swapBackend.refreshHistory()
                }
                GhostButton {
                    visible: historyRoot.receipts.length > 0
                    text: historyRoot.clearArmed ? "Confirm clear" : "Clear"
                    accented: historyRoot.clearArmed
                    Layout.preferredHeight: 32
                    font.pixelSize: Theme.fontCaption
                    onClicked: {
                        if (historyRoot.clearArmed) {
                            historyRoot.clearArmed = false
                            historyRoot.expandedIndex = -1
                            swapBackend.clearHistory()
                        } else {
                            historyRoot.clearArmed = true
                            clearDisarm.restart()
                        }
                    }
                }
            }

            Text {
                text: "Every completed, refunded, and failed swap on this profile — persisted across restarts. Click a row for the full receipt."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            EmptyState {
                visible: historyRoot.receipts.length === 0
                Layout.fillWidth: true
                Layout.topMargin: Theme.spacingLarge
                tone: Theme.accent
                title: "No swaps recorded yet"
                subtitle: "Receipts land here the moment a swap completes, refunds, or fails — and survive restarts."
            }

            // Receipt tape — newest first
            Repeater {
                model: historyRoot.receipts

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: Theme.spacingSmall

                    readonly property var receipt: modelData
                    readonly property bool expanded: historyRoot.expandedIndex === index

                    Rectangle {
                        Layout.fillWidth: true
                        implicitHeight: rowCol.implicitHeight + Theme.spacingNormal * 2
                        color: rowMouse.containsMouse || expanded
                               ? Qt.darker(Theme.surface, 1.05) : "transparent"

                        // Accent cursor bar on hover/expansion (tape idiom)
                        Rectangle {
                            anchors.left: parent.left
                            width: 3
                            height: parent.height
                            color: historyRoot.toneOf(receipt.status)
                            visible: rowMouse.containsMouse || expanded
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
                            id: rowMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: historyRoot.expandedIndex =
                                expanded ? -1 : index
                        }

                        ColumnLayout {
                            id: rowCol
                            anchors {
                                fill: parent
                                margins: Theme.spacingNormal
                            }
                            spacing: 6

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: Theme.spacingNormal

                                Rectangle {
                                    width: 8; height: 8; radius: 4
                                    color: historyRoot.toneOf(receipt.status)
                                }
                                Text {
                                    text: historyRoot.heroOf(receipt)
                                    color: Theme.textPrimary
                                    font.pixelSize: Theme.fontSmall
                                    font.bold: true
                                    font.family: Theme.monoFont
                                    elide: Text.ElideRight
                                    Layout.fillWidth: true
                                }
                                Text {
                                    text: (receipt.role || "").toUpperCase()
                                    color: receipt.role === "maker" ? Theme.success : Theme.accent
                                    font.pixelSize: Theme.fontMicro
                                    font.bold: true
                                    font.letterSpacing: 1
                                }
                                Text {
                                    text: historyRoot.stampOf(receipt)
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.fontCaption
                                    font.family: Theme.monoFont
                                }
                            }

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: Theme.spacingNormal

                                Text {
                                    text: receipt.hashlock
                                          ? "Hashlock " + receipt.hashlock.substring(0, 10) + "…"
                                          : (receipt.eth && receipt.eth.swap_id
                                             ? "Swap ID " + String(receipt.eth.swap_id).substring(0, 10) + "…"
                                             : "—")
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.fontDetail
                                    font.family: Theme.monoFont
                                }
                                Item { Layout.fillWidth: true }
                                Text {
                                    visible: receipt.error !== undefined && receipt.error !== null
                                    text: receipt.error || ""
                                    color: Theme.error
                                    font.pixelSize: Theme.fontCaption
                                    elide: Text.ElideRight
                                    Layout.maximumWidth: 320
                                }
                                Text {
                                    text: expanded ? "Collapse ▴" : "Receipt ▾"
                                    color: Theme.accent
                                    font.pixelSize: Theme.fontCaption
                                    font.bold: true
                                }
                            }
                        }
                    }

                    ReceiptCard {
                        visible: expanded
                        role: receipt.role === "maker" ? "maker" : "taker"
                        resultJson: expanded ? historyRoot.resultJsonOf(receipt) : ""
                        context: expanded ? historyRoot.contextOf(receipt) : null
                        showHistoryHint: false
                    }
                }
            }
        }
    }
}
