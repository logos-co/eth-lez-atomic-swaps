import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme
import SwapFormat

// Swap history — the durable archive of every completed / refunded /
// failed swap on this profile, fed by the plugin's per-profile JSONL
// journal through the receiptsJson PROP (swap-receipt/1 objects, newest
// first). Rows use the offer tape idiom (hairline separators, monospace
// data, status dot); clicking a row expands the full ReceiptCard.
PageScaffold {
    id: historyRoot

    title: "Swap history"
    subtitle: "Every completed, refunded and failed swap on this profile, kept "
              + "across restarts. Click a row for the full receipt."

    headerTrailingData: StatusChip {
        visible: historyRoot.receipts.length > 0
        text: historyRoot.receipts.length + " swap"
              + (historyRoot.receipts.length !== 1 ? "s" : "")
        status: "waiting"
        showDot: false
    }

    readonly property var receipts: {
        try {
            var arr = JSON.parse(swapBackend.receiptsJson || "[]")
            return Array.isArray(arr) ? arr : []
        } catch (e) {
            return []
        }
    }
    // Which receipt is open, identified by its hashlock rather than its row
    // number. The 5s re-read below rebuilds the list newest-first, so a receipt
    // appended by another process PREPENDS and shifts every index down: a
    // positional `expandedIndex` would silently leave a DIFFERENT swap open in
    // front of someone reading a receipt. Keying by identity makes the open row
    // follow its own swap. Falls back to the row index only when a receipt
    // somehow carries no hashlock.
    property string expandedKey: ""

    function receiptKey(receipt, index) {
        if (!receipt) return ""
        return receipt.hashlock ? String(receipt.hashlock) : ("#" + index)
    }
    property bool clearArmed: false
    property bool resetArmed: false

    function ethDisplayOf(r) {
        if (r.eth_amount_wei) return Format.weiToEth(r.eth_amount_wei)
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

    // Receipt status -> the app's five-state vocabulary. A refund is not a
    // failure: the safety net worked and the money came back, so it is
    // "attention" (worth a look), not "problem".
    function statusOf(status) {
        if (status === "completed") return "live"
        if (status === "refunded") return "attention"
        return "problem"
    }

    function toneOf(status) {
        return Theme.toneFor(historyRoot.statusOf(status))
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

    // Re-read the journal from disk while this tab is on screen.
    //
    // Swaps made by THIS instance already publish live — journalReceipt()
    // updates receiptsJson in-process before it touches disk — so this is
    // not about our own rows. It covers what the deleted Refresh button
    // covered: another process appending to the same journal, i.e. the
    // two-instance maker/taker flow this repo is dogfooded with. Without
    // it, a sibling's receipts need an app restart to show up.
    //
    // Declaring a Timer here used to be the #113 footgun — a non-visual object
    // ahead of the content Flickable stole ScrollView's content slot and
    // collapsed the layout. PageScaffold owns the Flickable and callers can
    // only append into its content column, so the hazard is no longer reachable
    // from a view and this no longer needs to hide inside the Flickable.
    Timer {
        interval: 5000
        repeat: true
        running: historyRoot.visible
        onTriggered: swapBackend.refreshHistory()
    }

    Timer {
        id: clearDisarm
        interval: 3000
        onTriggered: historyRoot.clearArmed = false
    }

    Timer {
        id: resetDisarm
        interval: 3000
        onTriggered: historyRoot.resetArmed = false
    }

    // Destructive page actions, right-aligned and away from the title. They
    // used to sit in the title row, which put "Reset app data" — the control
    // that deletes saved private keys — directly beside the page heading.
    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacingSmall

        Item { Layout.fillWidth: true }

        GhostButton {
            visible: historyRoot.receipts.length > 0
            text: historyRoot.clearArmed ? "Confirm clear" : "Clear"
            accented: historyRoot.clearArmed
            Layout.preferredHeight: 32
            font.pixelSize: Theme.fontCaption
            onClicked: {
                if (historyRoot.clearArmed) {
                    historyRoot.clearArmed = false
                    historyRoot.expandedKey = ""
                    swapBackend.clearHistory()
                } else {
                    historyRoot.clearArmed = true
                    clearDisarm.restart()
                }
            }
        }
        GhostButton {
            text: historyRoot.resetArmed ? "Confirm reset" : "Reset app data"
            accented: historyRoot.resetArmed
            Layout.preferredHeight: 32
            font.pixelSize: Theme.fontCaption
            onClicked: {
                if (historyRoot.resetArmed) {
                    historyRoot.resetArmed = false
                    swapBackend.resetConfig()
                } else {
                    historyRoot.resetArmed = true
                    resetDisarm.restart()
                }
            }
        }
    }

    Text {
        visible: historyRoot.resetArmed
        text: "This deletes the saved setup (including any private keys) and restores the built-in defaults. Your swap history is not affected."
        color: Theme.warning
        font.pixelSize: Theme.fontCaption
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

    // Receipt tape — newest first.
    //
    // Its own column at spacing 0, so consecutive rows abut and the hairline
    // separators do the dividing. Left in the page's 24px rhythm the rows
    // floated apart into a stack of cards, which is the opposite of a tape —
    // and made the separators look like stray rules.
    ColumnLayout {
        Layout.fillWidth: true
        spacing: 0

        Repeater {
            model: historyRoot.receipts

            ColumnLayout {
                Layout.fillWidth: true
                spacing: Theme.spacingSmall

                readonly property var receipt: modelData
                readonly property string rowKey: historyRoot.receiptKey(modelData, index)
                readonly property bool expanded: historyRoot.expandedKey !== ""
                                                 && historyRoot.expandedKey === rowKey

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
                        onClicked: historyRoot.expandedKey =
                            expanded ? "" : parent.parent.rowKey
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

                            StatusDot {
                                status: historyRoot.statusOf(receipt.status)
                                Layout.alignment: Qt.AlignVCenter
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
                                // Protocol roles are not user vocabulary: the
                                // person who held LEZ is the seller, the person who
                                // held ETH is the buyer. This tape said MAKER/TAKER
                                // while the receipt beneath it said seller/buyer.
                                text: receipt.role === "maker" ? "SOLD" : "BOUGHT"
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
                                // One truncation rule, via Format.shortHex — these
                                // were two hand-rolled `substring(0, 10) + "…"`
                                // head-only clips, which drop the tail a block
                                // explorer is compared against.
                                text: receipt.hashlock
                                      ? "Hashlock " + Format.shortHex(receipt.hashlock)
                                      : (receipt.eth && receipt.eth.swap_id
                                         ? "Swap ID " + Format.shortHex(receipt.eth.swap_id)
                                         : "—")
                                textFormat: Text.PlainText
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontDetail
                                font.family: Theme.monoFont
                            }
                            Item { Layout.fillWidth: true }
                            Text {
                                visible: receipt.error !== undefined && receipt.error !== null
                                // Journalled backend/chain text, not author copy.
                                textFormat: Text.PlainText
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
                    Layout.bottomMargin: expanded ? Theme.spacingNormal : 0
                    role: receipt.role === "maker" ? "maker" : "taker"
                    resultJson: expanded ? historyRoot.resultJsonOf(receipt) : ""
                    context: expanded ? historyRoot.contextOf(receipt) : null
                    showHistoryHint: false
                }
            }
        }
    }
}
