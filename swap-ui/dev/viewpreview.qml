// Dev-only visual harness for a view in a POSED state. NOT shipped.
//
// preview.qml renders the real shell, but with no backend every data-driven
// view is empty: the offer board's master/detail split, column alignment, ghost
// rows and whole right-hand trust surface are `visible: false`, and the history
// tape has no receipts. The states worth looking at are exactly the ones that
// need data.
//
// This supplies it. `swapBackend` is resolved by dynamic scope — a view reads
// the id declared by whichever document created it, which in the shipped app is
// Main.qml's facade and here is the writable stub below. Same mechanism as the
// real app, so the views run their real code paths (OfferFilter.classifyOffers,
// the keyed ListModel merge, the receipt adapters) against fabricated data.
//
//   qml -I swap-ui/src/qml swap-ui/dev/viewpreview.qml
//
// Override with -- -view=board|history -state=offers|empty -out=/tmp/x.png
// -w=1600 -h=1000. -hold=1 keeps the window open instead of screenshotting and
// quitting, for hover/resize/animation checks by hand.
import QtQuick
import QtQuick.Window
import SwapTheme
import "../src/qml"

Window {
    id: win

    function arg(name, fallback) {
        var args = Qt.application.arguments
        for (var i = 0; i < args.length; i++) {
            if (args[i].indexOf("-" + name + "=") === 0)
                return args[i].split("=")[1]
        }
        return fallback
    }

    readonly property string outPath: arg("out", "/tmp/swap-ui-view.png")
    readonly property string view: arg("view", "board")
    readonly property string state: arg("state", "offers")
    readonly property bool hold: arg("hold", "0") === "1"

    width: parseInt(arg("w", "1400"))
    height: parseInt(arg("h", "900"))
    visible: true
    color: "#171717"
    title: "swap-ui view preview — " + win.view

    // The canonical pinned venue. Offers naming anything else must ghost.
    readonly property string canonEth: "0x8636fe66b1d0d0a0f9d0e0c0b0a090807060504a"
    readonly property string canonLez:
        "b7a4c1d9e2f30516273849506172839405162738495061728394051627384950"

    // --- Stub facade ----------------------------------------------------
    // Mirrors Main.qml's `swapBackend` QtObject for exactly the members
    // OfferBoard reads, but writable so a state can be posed.
    QtObject {
        id: swapBackend

        property bool ready: true
        property string errorMessage: ""
        property bool running: false
        property string validationErrorsJson: "{}"

        property string ethRpcUrl: "https://sepolia.example/rpc"
        property string ethHtlcAddress: win.canonEth
        property string canonicalEthHtlcAddress: win.canonEth
        property string canonicalLezHtlcProgramId: win.canonLez
        property string lezSequencerUrl: "https://testnet.lez.logos.co"
        property string lezHtlcProgramId: win.canonLez

        property bool offersLoading: false
        property bool refundsLoading: false
        property bool makerRunning: false
        property bool takerRunning: false
        property bool autoAcceptRunning: false

        property bool messagingConnected: true
        property int messagingPeerCount: 6
        property bool messagingPeerCountKnown: true
        property string messagingHint: ""
        property string messagingRetrying: ""

        property string receiptsJson: win.state === "empty" ? "[]" : win.receipts()

        signal offersFetched(string offersJson)

        function fetchOffers() {}
        function publishOfferRequest() {}
        function validateConfig() {}
        function refreshHistory() {}
        function clearHistory() { swapBackend.receiptsJson = "[]" }
        function resetConfig() {}
        function acceptOfferAndStartTaker(offer) {
            console.log("[viewpreview] accept:", JSON.stringify(offer))
        }
    }

    // --- Fabricated receipts (swap-receipt/1) ---------------------------
    // One of each terminal status and both roles, so the tape shows every
    // status tone and both SOLD/BOUGHT labels in one shot.
    function receipts() {
        var now = Date.now()
        return JSON.stringify([
            { hashlock: pad("a1", 64), role: "taker", status: "completed",
              lez_amount: "2500", eth_amount_wei: "50000000000000000",
              started_ms: now - 400000, finished_ms: now - 380000,
              eth: { swap_id: pad("11", 64), lock_tx: pad("12", 64) },
              lez: { claim_tx: pad("13", 64) },
              network: { eth_chain_id: 11155111,
                         lez_sequencer: "https://testnet.lez.logos.co" } },
            { hashlock: pad("b2", 64), role: "maker", status: "completed",
              lez_amount: "9000", eth_amount_wei: "100000000000000000",
              started_ms: now - 90000000, finished_ms: now - 89000000,
              eth: { claim_tx: pad("21", 64) }, lez: { lock_tx: pad("22", 64) },
              network: { eth_chain_id: 11155111 } },
            { hashlock: pad("c3", 64), role: "taker", status: "refunded",
              lez_amount: "400", eth_amount_wei: "9000000000000000",
              started_ms: now - 180000000, finished_ms: now - 179000000,
              eth: { swap_id: pad("31", 64), refund_tx: pad("32", 64) },
              network: { eth_chain_id: 11155111 } },
            { hashlock: pad("d4", 64), role: "maker", status: "failed",
              lez_amount: "12", eth_amount_wei: "600000000000000",
              started_ms: now - 260000000, finished_ms: now - 259000000,
              error: "lez claim rejected: account not initialized",
              network: { eth_chain_id: 11155111 } }
        ])
    }

    // --- Fabricated wire offers -----------------------------------------
    // Shaped exactly like the delivery relay's payload so they survive
    // OfferFilter.offerWellFormed: 20-byte addresses, 32-byte hashlock /
    // program id, positive amounts, future timelocks.
    // 0x-prefixed like the real wire format: the board truncates with an
    // 8-char head, so whether the prefix is there changes what the preview
    // shows ("0xdeadbe…" vs "deadbe60…").
    function pad(prefix, hexChars) {
        var s = prefix.replace(/^0x/, "")
        while (s.length < hexChars) s += (s.length % 7).toString(16)
        return "0x" + s.substring(0, hexChars)
    }

    function offer(o) {
        var now = Math.floor(Date.now() / 1000)
        return {
            hashlock: pad(o.h, 64),
            lez_amount: o.lez,
            eth_amount: o.wei,
            maker_eth_address: pad(o.maker, 40),
            maker_lez_account: pad(o.makerLez || o.h, 64),
            lez_timelock: now + o.lezMin * 60,
            eth_timelock: now + o.ethMin * 60,
            lez_htlc_program_id: o.lezProg || win.canonLez,
            eth_htlc_address: o.ethAddr || win.canonEth,
            timestamp_ms: Date.now() - (o.ageSec || 0) * 1000
        }
    }

    function seedOffers() {
        var board = win.findByProp(win.contentItem, "maxGhostRows")
        if (!board) {
            console.log("[viewpreview] OfferBoard not found")
            return
        }
        board.mergeOffers(JSON.stringify({ offers: win.seedBatch() }))
    }

    function seedBatch() {
        return [
            // Fresh, mid-rate.
            offer({ h: "a1", lez: "2500", wei: "50000000000000000",
                    maker: "1a2b3c", lezMin: 120, ethMin: 60, ageSec: 2 }),
            // Best rate on the board -> should win the green ★.
            offer({ h: "b2", lez: "9000", wei: "100000000000000000",
                    maker: "4d5e6f", lezMin: 90, ethMin: 45, ageSec: 45 }),
            // Old + close to expiry -> faded row, red countdown.
            offer({ h: "c3", lez: "400", wei: "9000000000000000",
                    maker: "778899", lezMin: 4, ethMin: 2, ageSec: 900 }),
            // Sub-0.001 ETH -> exercises the Gwei branch of weiToEth.
            offer({ h: "d4", lez: "12", wei: "600000000000000",
                    maker: "aabbcc", lezMin: 200, ethMin: 100, ageSec: 300 }),
            // NON-CANONICAL venue -> must render as a blocked ghost row.
            offer({ h: "e5", lez: "999999", wei: "1000000000000000",
                    maker: "deadbe", lezMin: 300, ethMin: 150, ageSec: 20,
                    ethAddr: win.pad("badbad", 40) })
        ]
    }

    // Depth-first search for the board by a property only it declares.
    function findByProp(item, prop) {
        if (!item) return null
        if (item.hasOwnProperty(prop)) return item
        for (var i = 0; i < item.children.length; i++) {
            var found = win.findByProp(item.children[i], prop)
            if (found) return found
        }
        return null
    }

    // Main.qml paints this behind every view. grabToImage renders the item
    // tree only — Window.color is not in it — so without this the board's
    // transparent rows come out white in the screenshot.
    Rectangle {
        anchors.fill: parent
        color: Theme.background
    }

    OfferBoard {
        anchors.fill: parent
        visible: win.view === "board"
    }

    HistoryView {
        anchors.fill: parent
        visible: win.view === "history"
    }

    // Second delivery of the SAME batch plus one new offer. The board merges
    // into a keyed ListModel rather than rebuilding it, so a re-poll must not
    // duplicate rows and must not move the selection — the two things a
    // screenshot cannot show. Asserted in the log instead of by eye.
    property string selectionBeforeRepoll: ""
    property int rowsBeforeRepoll: 0

    function repollOffers() {
        var b = win.findByProp(win.contentItem, "maxGhostRows")
        if (!b) return
        win.selectionBeforeRepoll = b.selectedKey
        win.rowsBeforeRepoll = b.offerCount
        var again = win.seedBatch()
        again.push(offer({ h: "f6", lez: "77", wei: "3000000000000000",
                           maker: "c0ffee", lezMin: 150, ethMin: 75, ageSec: 0 }))
        b.mergeOffers(JSON.stringify({ offers: again }))
        var grew = b.offerCount - win.rowsBeforeRepoll
        console.log("[viewpreview] re-poll: rows",
                    win.rowsBeforeRepoll, "->", b.offerCount,
                    grew === 1 ? "(OK: only the new offer was added)"
                               : "(FAIL: expected exactly 1 new row)")
        console.log("[viewpreview] re-poll: selection",
                    b.selectedKey === win.selectionBeforeRepoll
                    ? "(OK: unchanged)"
                    : "(FAIL: moved from " + win.selectionBeforeRepoll
                      + " to " + b.selectedKey + ")")
        console.log("[viewpreview] selection is a blocked row?",
                    b.sel ? b.sel.blocked : "no selection")
    }

    Timer {
        interval: 600
        running: true
        repeat: false
        onTriggered: {
            if (win.view === "board" && win.state === "offers") {
                win.seedOffers()
                repollTimer.start()
            }
            if (!win.hold)
                grabTimer.start()
        }
    }

    Timer {
        id: repollTimer
        interval: 500
        repeat: false
        onTriggered: win.repollOffers()
    }

    // Long enough for the 1 Hz sweep to have cleared `isNew` on the seeded
    // rows that were fabricated old, so the shot shows a mix of pinged and
    // settled rows rather than five simultaneous NEW pings.
    Timer {
        id: grabTimer
        interval: 1600
        repeat: false
        onTriggered: win.contentItem.grabToImage(function (result) {
            result.saveToFile(win.outPath)
            Qt.callLater(Qt.quit)
        }, Qt.size(win.width, win.height))
    }
}
