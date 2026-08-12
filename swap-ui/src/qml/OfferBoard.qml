import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme
import "OfferFilter.js" as OfferFilter

// Live offer board — the app's home screen.
//
// Master-detail "trading dashboard": scannable offer tape on the left,
// selected-offer detail + one-click accept on the right, market status
// strip on top. Offers arrive by polling the delivery relay every few
// seconds (drains are destructive, so rows are merged into a ListModel
// keyed by hashlock — delegates persist across refreshes, which keeps
// hover/selection state and lets insert/remove animations work).
Item {
    id: board

    // --- Constants -----------------------------------------------------
    readonly property int pollIntervalMs: 5000
    readonly property int freshMs: 12000          // "NEW" ping window
    readonly property int expiredGraceSec: 15     // keep expired rows briefly
    readonly property int tabConfig: 1
    readonly property int tabMaker: 2
    readonly property int tabTaker: 3
    // Spam caps (defense-in-depth on top of the backend cache's own caps).
    // maxOffers bounds the total rendered set so a spammer can't grow the
    // board unbounded; maxGhostRows bounds the "blocked — unsafe" ghost rows
    // so a flood of non-canonical offers can't bury honest ones (beyond the
    // cap, venue-mismatch offers are silently dropped, not ghosted).
    readonly property int maxOffers: 200
    readonly property int maxGhostRows: 4

    // --- Live state ----------------------------------------------------
    property int tick: 0                // 1 Hz clock driving countdowns/fades
    property int modelRev: 0            // bumped on any model change (sel dep)
    property string selectedKey: ""
    property bool accepting: false
    property string acceptError: ""
    property bool validationRequested: false
    // True once a validateConfig round-trip has answered. The host inits
    // validationErrorsJson to "{}" and the replica setter no-ops on equal
    // values, so a valid config produces no change signal — the fallback
    // timer below settles that case instead. Gating configReady on this
    // (not on the request) stops the ready-chip flash and the spurious
    // fetchOffers during the round-trip window on a fresh install.
    property bool validationSettled: false
    property real bestRate: 0           // best LEZ-per-ETH on the board
    // How many "blocked — unsafe" ghost rows are currently on the board
    // (recomputed on every model change). Drives the subtle header counter —
    // the calm "the app is protecting you" signal — no addresses, no styling.
    property int blockedCount: 0

    readonly property var validationErrors: {
        try { return JSON.parse(swapBackend.validationErrorsJson || "{}") }
        catch (e) { return {} }
    }
    readonly property int configIssueCount: Object.keys(validationErrors).length
    readonly property bool configReady: validationSettled && configIssueCount === 0
    // Network constants only — mirrors the backend's validateForBrowse()
    // (swap_ui_plugin.cpp). All four are pre-filled with public-testnet
    // defaults out of the box (see feat/sane-testnet-defaults), so a
    // zero-config fresh install can browse the market immediately; only
    // *accepting* an offer needs the full configReady (credentials, amounts,
    // timelocks — see canAccept below).
    readonly property bool browseReady: swapBackend.ethRpcUrl !== ""
        && swapBackend.ethHtlcAddress !== ""
        && swapBackend.lezSequencerUrl !== ""
        && swapBackend.lezHtlcProgramId !== ""
    readonly property bool marketLive: swapBackend.ready
        && swapBackend.messagingConnected
        && browseReady

    // The delivery node is up + subscribed but has a CONFIRMED zero fleet
    // peers: nothing published anywhere can reach this board. Distinct from
    // "count not known yet" (messagingPeerCountKnown false), which stays on
    // the optimistic waking-up copy. Gated on messagingHint so the alarm only
    // fires once the backend's post-start grace window has passed — 0 peers
    // in the first seconds of a healthy startup is normal dialing.
    readonly property bool fleetIsolated: swapBackend.messagingConnected
        && swapBackend.messagingPeerCountKnown
        && swapBackend.messagingPeerCount === 0
        && swapBackend.messagingHint !== ""

    // Selected offer (plain object copy; depends on modelRev + selectedKey)
    readonly property var sel: {
        void board.modelRev
        return board.selectedKey === "" ? null : findOffer(board.selectedKey)
    }

    readonly property bool selExpired: {
        void board.tick
        return sel !== null && remainingSec(sel) <= 0
    }

    readonly property bool canAccept: sel !== null
        // A ghosted (venue-mismatch) offer is never acceptable — the accept
        // path stays disabled; the accept-time venue check is the true gate.
        && !sel.blocked
        && !selExpired
        && swapBackend.ready
        && swapBackend.messagingConnected
        && configReady
        && !swapBackend.running
        // No accept while a fetch is in flight: a failing fetch would land
        // in errorMessage mid-accept and be misread as an accept failure.
        && !swapBackend.offersLoading
        && !accepting

    onMarketLiveChanged: {
        if (marketLive)
            swapBackend.fetchOffers()
    }

    // --- Model ---------------------------------------------------------
    ListModel { id: offersModel }

    readonly property int offerCount: offersModel.count

    function offerKey(o) {
        if (o.hashlock && o.hashlock !== "")
            return o.hashlock
        return o.maker_eth_address + ":" + o.lez_amount + ":" + o.eth_amount
    }

    function ghostRowCount() {
        var n = 0
        for (var i = 0; i < offersModel.count; i++)
            if (offersModel.get(i).blocked) n++
        return n
    }

    // Merge a freshly-fetched batch. All safety classification (malformed drop,
    // venue-mismatch ghosting, ghost + spam caps) lives in the pure, unit-tested
    // OfferFilter.classifyOffers; this only applies its verdict to the model.
    function mergeOffers(offersJson) {
        var obj = {}
        try { obj = JSON.parse(offersJson || "{}") } catch (e) { return }
        if (!obj.offers || obj.offers.length === 0)
            return

        var existingKeys = []
        for (var i = 0; i < offersModel.count; i++)
            existingKeys.push(offersModel.get(i).key)
        var ghosts = ghostRowCount()

        var admit = OfferFilter.classifyOffers(obj.offers, {
            nowSec: Math.floor(Date.now() / 1000),
            canonicalEth: swapBackend.canonicalEthHtlcAddress,
            canonicalLez: swapBackend.canonicalLezHtlcProgramId,
            existingKeys: existingKeys,
            ghostCount: ghosts,
            honestCount: offersModel.count - ghosts,
            maxOffers: board.maxOffers,
            maxGhostRows: board.maxGhostRows,
            keyOf: board.offerKey
        })
        if (admit.length === 0)
            return

        for (var a = 0; a < admit.length; a++) {
            var o = admit[a].offer
            // Honest overflow is capped in classifyOffers, but evict the oldest
            // honest row here too so a long-lived board stays bounded as fresh
            // honest offers arrive.
            if (!admit[a].blocked
                    && offersModel.count - ghostRowCount() >= board.maxOffers)
                evictOldestHonest()
            offersModel.insert(0, {
                key: admit[a].key,
                hashlock: String(o.hashlock || ""),
                lezAmount: String(o.lez_amount),
                ethAmountWei: String(o.eth_amount),
                makerEth: String(o.maker_eth_address || ""),
                makerLez: String(o.maker_lez_account || ""),
                lezTimelock: Number(o.lez_timelock || 0),
                ethTimelock: Number(o.eth_timelock || 0),
                lezProgramId: String(o.lez_htlc_program_id || ""),
                ethHtlcAddr: String(o.eth_htlc_address || ""),
                receivedMs: Number(o.timestamp_ms || Date.now()),
                isNew: true,
                blocked: admit[a].blocked
            })
        }
        afterModelChange()
    }

    // Remove the oldest honest (non-blocked) row — the bottom-most non-ghost.
    function evictOldestHonest() {
        for (var i = offersModel.count - 1; i >= 0; i--) {
            if (!offersModel.get(i).blocked) {
                offersModel.remove(i)
                return
            }
        }
    }

    // 1 Hz sweep: expire the NEW flag, prune long-expired offers.
    function sweep() {
        var nowSec = Math.floor(Date.now() / 1000)
        var changed = false
        for (var i = offersModel.count - 1; i >= 0; i--) {
            var o = offersModel.get(i)
            var expiry = Math.min(o.lezTimelock, o.ethTimelock)
            if (expiry > 0 && nowSec > expiry + board.expiredGraceSec) {
                offersModel.remove(i)
                changed = true
                continue
            }
            if (o.isNew && Date.now() - o.receivedMs > board.freshMs) {
                offersModel.setProperty(i, "isNew", false)
                changed = true
            }
        }
        if (changed)
            afterModelChange()
    }

    function afterModelChange() {
        // Best rate (only meaningful with competition). Ghosted (blocked)
        // rows are excluded — a non-canonical offer must never set the market
        // rate or win the ★ best-deal badge.
        var best = 0
        var blocked = 0
        for (var i = 0; i < offersModel.count; i++) {
            var row = offersModel.get(i)
            if (row.blocked) { blocked++; continue }
            var r = rateOf(row)
            if (r > best) best = r
        }
        board.bestRate = best
        board.blockedCount = blocked
        // Keep selection stable by key. Auto-select the freshest offer only
        // when nothing was selected; if the selected offer was pruned, clear
        // instead — silently swapping the detail pane (and an enabled Accept)
        // to a different offer under the cursor invites a mis-click.
        var found = false
        for (var j = 0; j < offersModel.count; j++) {
            if (offersModel.get(j).key === board.selectedKey) { found = true; break }
        }
        if (!found)
            board.selectedKey = (board.selectedKey === "" && offersModel.count > 0)
                ? offersModel.get(0).key : ""
        board.modelRev++
    }

    function findOffer(key) {
        for (var i = 0; i < offersModel.count; i++) {
            var o = offersModel.get(i)
            if (o.key === key) {
                return {
                    key: o.key, hashlock: o.hashlock,
                    lezAmount: o.lezAmount, ethAmountWei: o.ethAmountWei,
                    makerEth: o.makerEth, makerLez: o.makerLez,
                    lezTimelock: o.lezTimelock, ethTimelock: o.ethTimelock,
                    lezProgramId: o.lezProgramId, ethHtlcAddr: o.ethHtlcAddr,
                    receivedMs: o.receivedMs, isNew: o.isNew,
                    blocked: o.blocked
                }
            }
        }
        return null
    }

    // Rebuild the wire-format offer object the accept path expects.
    function wireOffer(o) {
        return {
            hashlock: o.hashlock,
            lez_amount: o.lezAmount,
            eth_amount: o.ethAmountWei,
            maker_eth_address: o.makerEth,
            maker_lez_account: o.makerLez,
            lez_timelock: o.lezTimelock,
            eth_timelock: o.ethTimelock,
            lez_htlc_program_id: o.lezProgramId,
            eth_htlc_address: o.ethHtlcAddr,
            timestamp_ms: o.receivedMs
        }
    }

    function acceptSelected() {
        if (!canAccept || sel === null)
            return
        var offer = wireOffer(sel)
        board.acceptError = ""
        board.accepting = true
        takerView.acceptedOffer = offer
        takerView.swapCompleted = false
        swapBackend.acceptOfferAndStartTaker(offer)
    }

    // --- Formatting helpers --------------------------------------------
    function weiToEth(wei) {
        var n = Number(wei)
        if (isNaN(n) || n === 0) return "0 ETH"
        var eth = n / 1e18
        if (eth >= 0.001) return eth.toFixed(6).replace(/\.?0+$/, '') + " ETH"
        var gwei = n / 1e9
        if (gwei >= 1) return gwei.toFixed(4).replace(/\.?0+$/, '') + " Gwei"
        return wei + " wei"
    }

    function rateOf(o) {
        var eth = Number(o.ethAmountWei) / 1e18
        var lez = Number(o.lezAmount)
        if (!(eth > 0) || !(lez > 0)) return 0
        return lez / eth
    }

    function fmtRate(r) {
        if (!(r > 0)) return "—"
        if (r >= 100) return r.toFixed(0)
        if (r >= 1) return r.toFixed(2)
        return r.toFixed(6)
    }

    function shortHex(s, head, tail) {
        if (!s || s.length <= head + tail + 3) return s || "—"
        return s.substring(0, head) + "…" + s.substring(s.length - tail)
    }

    function fmtAge(receivedMs) {
        var sec = Math.max(0, Math.floor((Date.now() - receivedMs) / 1000))
        if (sec < 60) return sec + "s"
        var min = Math.floor(sec / 60)
        if (min < 60) return min + "m"
        return Math.floor(min / 60) + "h " + (min % 60) + "m"
    }

    function remainingSec(o) {
        var expiry = Math.min(o.lezTimelock, o.ethTimelock)
        if (expiry <= 0) return 0
        return expiry - Math.floor(Date.now() / 1000)
    }

    function fmtRemaining(sec) {
        if (sec <= 0) return "expired"
        if (sec < 60) return sec + "s"
        if (sec < 600) return Math.floor(sec / 60) + "m " + (sec % 60) + "s"
        var min = Math.floor(sec / 60)
        if (min < 60) return min + "m"
        return Math.floor(min / 60) + "h " + (min % 60) + "m"
    }

    function rampColor(sec) {
        if (sec <= 0) return Theme.error
        if (sec < 180) return Theme.error
        if (sec < 600) return Theme.warning
        return Theme.textSecondary
    }

    function ageOpacity(receivedMs) {
        var ageSec = (Date.now() - receivedMs) / 1000
        return Math.max(0.55, 1 - (ageSec / 600) * 0.45)
    }

    // --- Clocks --------------------------------------------------------
    Timer {
        interval: 1000
        running: board.visible
        repeat: true
        onTriggered: { board.tick++; board.sweep() }
    }

    Timer {
        id: pollTimer
        interval: board.pollIntervalMs
        running: board.marketLive
        repeat: true
        onTriggered: {
            if (board.marketLive && !swapBackend.offersLoading
                    && !swapBackend.running && !board.accepting)
                swapBackend.fetchOffers()
        }
    }

    // Run config validation once the backend comes up so readiness (and the
    // Config tab's field-level hints) reflect reality without user action.
    Timer {
        interval: 400
        running: swapBackend.ready && !board.validationRequested
        repeat: false
        onTriggered: {
            board.validationRequested = true
            swapBackend.validateConfig()
            validationSettleFallback.start()
        }
    }

    // Settles validation when the answer is "no errors": equal-value writes
    // to validationErrorsJson emit no change signal, so a valid config would
    // otherwise never flip validationSettled. The round-trip is local IPC
    // (ms), so by 1.5s any error answer has long since landed.
    Timer {
        id: validationSettleFallback
        interval: 1500
        repeat: false
        onTriggered: board.validationSettled = true
    }

    // Backstop for accept attempts that produce neither takerRunningChanged
    // nor an errorMessage *change*: the host's startTaker early-returns
    // silently when the replica's running state is stale, and repeated
    // failures reuse identical error strings the replica setter won't
    // re-emit. Without this, "Starting swap…" sticks forever and the
    // !accepting guard halts offer polling for the session.
    Timer {
        id: acceptTimeout
        interval: 20000
        repeat: false
        running: board.accepting
        onTriggered: {
            if (board.accepting && !swapBackend.takerRunning) {
                board.accepting = false
                board.acceptError = "The swap did not start. Check messaging and configuration, then try again."
                takerView.acceptedOffer = null
            }
        }
    }

    // Fetch balances once config is complete (header shows them app-wide).
    property bool balancesRequested: false
    Timer {
        interval: 600
        running: !board.balancesRequested && board.configReady && swapBackend.ready
            && swapBackend.ethBalance === "" && !swapBackend.balancesLoading
        repeat: false
        onTriggered: {
            board.balancesRequested = true
            swapBackend.fetchBalances()
        }
    }

    Connections {
        target: swapBackend

        function onOffersFetched(offersJson) {
            board.mergeOffers(offersJson)
        }

        function onValidationErrorsJsonChanged() {
            board.validationSettled = true
        }

        function onTakerRunningChanged() {
            if (swapBackend.takerRunning && board.accepting) {
                board.accepting = false
                tabBar.currentIndex = board.tabTaker
            }
        }

        function onErrorMessageChanged() {
            if (board.accepting && swapBackend.errorMessage !== "") {
                board.accepting = false
                board.acceptError = swapBackend.errorMessage
                takerView.acceptedOffer = null
            }
        }
    }

    // ====================================================================
    // Layout
    // ====================================================================
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // --- Market status strip ---------------------------------------
        Rectangle {
            Layout.fillWidth: true
            height: 44
            color: Theme.surface

            RowLayout {
                anchors {
                    fill: parent
                    leftMargin: Theme.spacingLarge
                    rightMargin: Theme.spacingLarge
                }
                spacing: Theme.spacingNormal

                // Live dot
                Rectangle {
                    width: 8; height: 8; radius: 4
                    color: board.marketLive ? Theme.success : Theme.warning

                    SequentialAnimation on opacity {
                        running: board.marketLive
                        loops: Animation.Infinite
                        NumberAnimation { from: 1.0; to: 0.35; duration: 900 }
                        NumberAnimation { from: 0.35; to: 1.0; duration: 900 }
                    }
                }

                Text {
                    text: "LIVE MARKET"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontSmall
                    font.bold: true
                    font.letterSpacing: 1.5
                }

                Text {
                    // Count only the tradable (non-ghost) offers here; the
                    // blocked ones get their own calm counter below.
                    readonly property int liveCount: offersModel.count - board.blockedCount
                    text: liveCount === 1 ? "1 offer" : liveCount + " offers"
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontSmall
                }

                // Subtle "the app is protecting you" signal: how many offers
                // were flagged unsafe (non-canonical venue) and rendered as
                // disabled ghost rows. No addresses, no scary styling — just a
                // muted count so the board is transparent, never tempting.
                Text {
                    visible: board.blockedCount > 0
                    text: "· " + board.blockedCount + " blocked"
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontSmall
                    textFormat: Text.PlainText
                }

                Item { Layout.fillWidth: true }

                // Poll heartbeat: bar drains between scans (graft: ticker spec)
                RowLayout {
                    spacing: Theme.spacingSmall
                    visible: board.marketLive

                    Text {
                        text: swapBackend.offersLoading ? "scanning…" : "next scan"
                        color: Theme.textMuted
                        font.pixelSize: 11
                    }
                    Item {
                        width: 56; height: 3

                        Rectangle {
                            anchors.fill: parent
                            radius: 1.5
                            color: Theme.surfaceLight
                        }
                        Rectangle {
                            id: drainBar
                            height: parent.height
                            radius: 1.5
                            color: Theme.accent
                            width: parent.width

                            NumberAnimation on width {
                                running: pollTimer.running
                                loops: Animation.Infinite
                                from: 56; to: 0
                                duration: board.pollIntervalMs
                            }
                        }
                    }
                }

                // Config readiness chip
                Rectangle {
                    implicitWidth: configChipRow.implicitWidth + Theme.spacingNormal
                    implicitHeight: 24
                    radius: 12
                    color: configChipMouse.containsMouse ? Theme.surfaceLight : "transparent"
                    border.color: board.configReady ? Theme.success : Theme.warning
                    border.width: 1

                    RowLayout {
                        id: configChipRow
                        anchors.centerIn: parent
                        spacing: 6

                        Rectangle {
                            width: 6; height: 6; radius: 3
                            color: board.configReady ? Theme.success : Theme.warning
                        }
                        Text {
                            text: board.configReady
                                  ? "Config ready"
                                  : (board.validationRequested
                                     ? board.configIssueCount + " config issue"
                                       + (board.configIssueCount === 1 ? "" : "s")
                                     : "Checking config…")
                            color: board.configReady ? Theme.success : Theme.warning
                            font.pixelSize: 11
                        }
                    }

                    MouseArea {
                        id: configChipMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: tabBar.currentIndex = board.tabConfig
                    }
                }

                // Maker CTA
                Rectangle {
                    implicitWidth: makeOfferText.implicitWidth + Theme.spacingNormal
                    implicitHeight: 24
                    radius: 12
                    color: makeOfferMouse.containsMouse ? Theme.surfaceLight : "transparent"
                    border.color: Theme.accent
                    border.width: 1

                    Text {
                        id: makeOfferText
                        anchors.centerIn: parent
                        text: "+ Make an offer"
                        color: Theme.accent
                        font.pixelSize: 11
                        font.bold: true
                    }
                    MouseArea {
                        id: makeOfferMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: tabBar.currentIndex = board.tabMaker
                    }
                }
            }

            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width
                height: 1
                color: Theme.border
            }
        }

        // --- Content ---------------------------------------------------
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            // Full-bleed empty / connecting / setup states (graft: card spec)
            Item {
                anchors.fill: parent
                visible: offersModel.count === 0

                ColumnLayout {
                    anchors.centerIn: parent
                    width: Math.min(parent.width - Theme.spacingXLarge * 2, 440)
                    spacing: Theme.spacingNormal

                    // Pulsing beacon
                    Item {
                        Layout.alignment: Qt.AlignHCenter
                        width: 48; height: 48

                        Rectangle {
                            id: beaconRing
                            anchors.centerIn: parent
                            width: 16; height: 16; radius: width / 2
                            color: "transparent"
                            border.width: 2
                            border.color: (board.marketLive && !board.fleetIsolated) ? Theme.accent : Theme.warning

                            ParallelAnimation {
                                running: offersModel.count === 0 && board.visible
                                loops: Animation.Infinite
                                NumberAnimation {
                                    target: beaconRing; property: "width"
                                    from: 16; to: 48; duration: 1800
                                    easing.type: Easing.OutQuad
                                }
                                NumberAnimation {
                                    target: beaconRing; property: "height"
                                    from: 16; to: 48; duration: 1800
                                    easing.type: Easing.OutQuad
                                }
                                NumberAnimation {
                                    target: beaconRing; property: "opacity"
                                    from: 0.9; to: 0.0; duration: 1800
                                }
                            }
                        }
                        Rectangle {
                            anchors.centerIn: parent
                            width: 10; height: 10; radius: 5
                            color: (board.marketLive && !board.fleetIsolated) ? Theme.accent : Theme.warning
                        }
                    }

                    Text {
                        Layout.alignment: Qt.AlignHCenter
                        text: {
                            if (!swapBackend.ready)
                                return "Starting swap engine…"
                            // browseReady (network constants), not configReady
                            // (full trade config) — browsing the market never
                            // needs credentials, amounts, or timelocks. See
                            // feat/browse-before-config. Pre-filled testnet
                            // defaults mean this branch is rarely reached.
                            if (!board.browseReady)
                                return "Finish network setup to browse"
                            if (!swapBackend.messagingConnected)
                                return "Connecting to the swap network…"
                            if (board.fleetIsolated)
                                return "Connected, but no fleet peers"
                            if (swapBackend.messagingHint !== "")
                                return "Delivery needs attention"
                            return "No offers on the board yet"
                        }
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontTitle
                        font.bold: true
                    }

                    Text {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        text: {
                            if (!swapBackend.ready)
                                return "The backend module is loading."
                            if (!board.browseReady)
                                return "Add your ETH RPC / LEZ sequencer details in Config to browse the market."
                            if (!swapBackend.messagingConnected)
                                return swapBackend.messagingRetrying
                                       ? "Waiting for the delivery fleet. Offers will stream in as soon as a peer is found."
                                       : "Delivery is starting automatically."
                            if (board.fleetIsolated)
                                return "Your delivery node is running and subscribed, but attached to zero fleet peers — offers cannot arrive. Check that the delivery_module is up to date in the module manager, then restart Basecamp."
                            if (swapBackend.messagingHint !== "")
                                return swapBackend.messagingHint
                            // Browsing never required Config; only accepting
                            // an offer does. Nudge the still-unconfigured case
                            // toward set up to *trade*, not "to see offers" —
                            // offers are already visible at this point.
                            return board.configReady
                                   ? "The market is waking up — offers appear here the moment makers publish them."
                                   : "The market is waking up — offers appear here the moment makers publish them. Set up to trade in Config once you see one you like."
                        }
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontNormal
                    }

                    Button {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 200
                        Layout.preferredHeight: 42
                        Layout.topMargin: Theme.spacingSmall
                        visible: swapBackend.ready
                                 && (board.configReady || board.validationRequested)
                        text: board.configReady ? "Make an offer" : "Open Config"
                        font.pixelSize: Theme.fontNormal
                        font.bold: true

                        background: Rectangle {
                            color: parent.hovered ? Theme.accentHover : Theme.accent
                            radius: Theme.radiusNormal
                        }
                        contentItem: Text {
                            text: parent.text
                            color: "#ffffff"
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                            font: parent.font
                        }
                        onClicked: tabBar.currentIndex =
                            board.configReady ? board.tabMaker : board.tabConfig
                    }

                    Text {
                        Layout.alignment: Qt.AlignHCenter
                        visible: board.marketLive
                        text: "scanning every " + (board.pollIntervalMs / 1000) + "s"
                        color: Theme.textMuted
                        font.pixelSize: 11
                    }
                }
            }

            // Master-detail split
            RowLayout {
                anchors.fill: parent
                visible: offersModel.count > 0
                spacing: 0

                // --- Left: offer tape ----------------------------------
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 0

                    // Column header
                    Rectangle {
                        Layout.fillWidth: true
                        height: 30
                        color: Theme.background

                        RowLayout {
                            anchors {
                                fill: parent
                                leftMargin: Theme.spacingLarge
                                rightMargin: Theme.spacingLarge
                            }
                            spacing: Theme.spacingNormal

                            Item { width: 12 }
                            Text {
                                Layout.preferredWidth: 150
                                text: "OFFER"
                                color: Theme.textMuted
                                font.pixelSize: 11; font.bold: true
                                font.letterSpacing: 1
                            }
                            Text {
                                Layout.preferredWidth: 96
                                text: "RATE LEZ/ETH"
                                color: Theme.textMuted
                                font.pixelSize: 11; font.bold: true
                                font.letterSpacing: 1
                            }
                            Text {
                                Layout.fillWidth: true
                                text: "MAKER"
                                color: Theme.textMuted
                                font.pixelSize: 11; font.bold: true
                                font.letterSpacing: 1
                            }
                            Text {
                                Layout.preferredWidth: 44
                                horizontalAlignment: Text.AlignRight
                                text: "AGE"
                                color: Theme.textMuted
                                font.pixelSize: 11; font.bold: true
                                font.letterSpacing: 1
                            }
                            Text {
                                Layout.preferredWidth: 72
                                horizontalAlignment: Text.AlignRight
                                text: "EXPIRES"
                                color: Theme.textMuted
                                font.pixelSize: 11; font.bold: true
                                font.letterSpacing: 1
                            }
                        }

                        Rectangle {
                            anchors.bottom: parent.bottom
                            width: parent.width
                            height: 1
                            color: Theme.border
                        }
                    }

                    ListView {
                        id: offerList
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        model: offersModel
                        boundsBehavior: Flickable.StopAtBounds

                        add: Transition {
                            NumberAnimation {
                                property: "opacity"
                                from: 0; to: 1; duration: 350
                            }
                        }
                        displaced: Transition {
                            NumberAnimation {
                                properties: "y"; duration: 220
                                easing.type: Easing.OutQuad
                            }
                        }
                        remove: Transition {
                            NumberAnimation {
                                property: "opacity"
                                to: 0; duration: 200
                            }
                        }

                        delegate: Rectangle {
                            id: offerRow
                            width: offerList.width
                            height: 48

                            readonly property bool selected: model.key === board.selectedKey
                            readonly property real rowRate: board.rateOf(model)
                            readonly property bool bestDeal: offersModel.count > 1
                                && board.bestRate > 0
                                && Math.abs(rowRate - board.bestRate) < 1e-9
                            readonly property int remain: {
                                void board.tick
                                return board.remainingSec(model)
                            }

                            color: selected
                                   ? Theme.surfaceLight
                                   : (rowMouse.containsMouse
                                      ? Qt.darker(Theme.surface, 1.05)
                                      : "transparent")

                            // Selection accent bar
                            Rectangle {
                                anchors.left: parent.left
                                width: 3
                                height: parent.height
                                color: Theme.accent
                                visible: offerRow.selected
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
                                onClicked: board.selectedKey = model.key
                            }

                            RowLayout {
                                id: rowContent
                                anchors {
                                    fill: parent
                                    leftMargin: Theme.spacingLarge
                                    rightMargin: Theme.spacingLarge
                                }
                                spacing: Theme.spacingNormal

                                // Age-fade lives on the content, not the
                                // delegate root, so it never fights the
                                // ListView add/remove transitions.
                                opacity: {
                                    void board.tick
                                    // Ghosted (venue-blocked) rows read as
                                    // clearly de-emphasised — present, but not
                                    // a live trade.
                                    if (model.blocked) return 0.5
                                    return offerRow.remain <= 0
                                           ? 0.35
                                           : board.ageOpacity(model.receivedMs)
                                }
                                Behavior on opacity {
                                    NumberAnimation { duration: 400 }
                                }

                                // Freshness ping (graft: card spec)
                                Item {
                                    width: 12; height: 12

                                    Rectangle {
                                        id: pingRing
                                        anchors.centerIn: parent
                                        width: 6; height: 6; radius: width / 2
                                        color: "transparent"
                                        border.width: 1
                                        border.color: Theme.success
                                        visible: model.isNew

                                        ParallelAnimation {
                                            running: model.isNew && board.visible
                                            loops: Animation.Infinite
                                            NumberAnimation {
                                                target: pingRing; property: "width"
                                                from: 6; to: 14; duration: 1200
                                            }
                                            NumberAnimation {
                                                target: pingRing; property: "height"
                                                from: 6; to: 14; duration: 1200
                                            }
                                            NumberAnimation {
                                                target: pingRing; property: "opacity"
                                                from: 0.9; to: 0; duration: 1200
                                            }
                                        }
                                    }
                                    Rectangle {
                                        anchors.centerIn: parent
                                        width: 5; height: 5; radius: 2.5
                                        color: model.isNew ? Theme.success : Theme.textMuted
                                        opacity: model.isNew ? 1.0 : 0.4
                                    }
                                }

                                // Pair amounts
                                RowLayout {
                                    Layout.preferredWidth: 150
                                    spacing: 6

                                    Text {
                                        textFormat: Text.PlainText
                                        text: model.lezAmount + " LEZ"
                                        color: Theme.textPrimary
                                        font.pixelSize: Theme.fontSmall
                                        font.bold: true
                                        font.family: Theme.monoFont
                                    }
                                    Text {
                                        text: "⇄"
                                        color: Theme.textMuted
                                        font.pixelSize: Theme.fontSmall
                                    }
                                    Text {
                                        Layout.fillWidth: true
                                        textFormat: Text.PlainText
                                        text: board.weiToEth(model.ethAmountWei)
                                        color: Theme.textPrimary
                                        font.pixelSize: Theme.fontSmall
                                        font.family: Theme.monoFont
                                        elide: Text.ElideRight
                                    }
                                }

                                // Rate (best deal highlighted; graft: ticker).
                                // For a ghosted row the rate/★ is replaced by a
                                // calm "blocked — unsafe" badge: the offer's
                                // economics are irrelevant, the point is that
                                // the app refused it.
                                Text {
                                    Layout.preferredWidth: 96
                                    textFormat: Text.PlainText
                                    text: model.blocked
                                          ? "⚠ blocked"
                                          : board.fmtRate(offerRow.rowRate)
                                            + (offerRow.bestDeal ? " ★" : "")
                                    color: model.blocked
                                           ? Theme.warning
                                           : (offerRow.bestDeal ? Theme.success : Theme.textSecondary)
                                    font.pixelSize: Theme.fontSmall
                                    font.family: Theme.monoFont
                                    font.bold: offerRow.bestDeal || model.blocked
                                }

                                // Maker
                                Text {
                                    Layout.fillWidth: true
                                    textFormat: Text.PlainText
                                    text: board.shortHex(model.makerEth, 8, 4)
                                    color: Theme.textMuted
                                    font.pixelSize: 12
                                    font.family: Theme.monoFont
                                    elide: Text.ElideRight
                                }

                                // Age
                                Text {
                                    Layout.preferredWidth: 44
                                    horizontalAlignment: Text.AlignRight
                                    text: {
                                        void board.tick
                                        return board.fmtAge(model.receivedMs)
                                    }
                                    color: Theme.textMuted
                                    font.pixelSize: 11
                                    font.family: Theme.monoFont
                                }

                                // Expiry countdown
                                Text {
                                    Layout.preferredWidth: 72
                                    horizontalAlignment: Text.AlignRight
                                    text: board.fmtRemaining(offerRow.remain)
                                    color: board.rampColor(offerRow.remain)
                                    font.pixelSize: 11
                                    font.family: Theme.monoFont
                                    font.strikeout: offerRow.remain <= 0
                                }
                            }
                        }
                    }

                    // Advertisement disclaimer
                    Rectangle {
                        Layout.fillWidth: true
                        height: 26
                        color: Theme.background

                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.left: parent.left
                            anchors.leftMargin: Theme.spacingLarge
                            text: "Offers are advertisements — a swap completes only if the maker is live."
                            color: Theme.textMuted
                            font.pixelSize: 11
                        }
                    }
                }

                // Divider
                Rectangle {
                    Layout.fillHeight: true
                    width: 1
                    color: Theme.border
                }

                // --- Right: detail + accept ----------------------------
                Rectangle {
                    Layout.preferredWidth: 340
                    Layout.fillHeight: true
                    color: Theme.surface

                    // Nothing selected
                    Text {
                        anchors.centerIn: parent
                        visible: board.sel === null
                        text: "Select an offer to inspect"
                        color: Theme.textMuted
                        font.pixelSize: Theme.fontNormal
                    }

                    Flickable {
                        anchors.fill: parent
                        visible: board.sel !== null
                        contentHeight: detailCol.implicitHeight + Theme.spacingLarge * 2
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds

                        ColumnLayout {
                            id: detailCol
                            anchors {
                                top: parent.top
                                left: parent.left
                                right: parent.right
                                margins: Theme.spacingLarge
                            }
                            spacing: Theme.spacingSmall

                            RowLayout {
                                Layout.fillWidth: true

                                Text {
                                    text: "OFFER"
                                    color: Theme.textMuted
                                    font.pixelSize: 11
                                    font.bold: true
                                    font.letterSpacing: 2
                                }
                                Rectangle {
                                    visible: board.sel !== null && board.sel.isNew && !board.sel.blocked
                                    implicitWidth: newChipText.implicitWidth + 12
                                    implicitHeight: 16
                                    radius: 8
                                    color: "transparent"
                                    border.color: Theme.success
                                    border.width: 1

                                    Text {
                                        id: newChipText
                                        anchors.centerIn: parent
                                        text: "NEW"
                                        color: Theme.success
                                        font.pixelSize: 9
                                        font.bold: true
                                    }
                                }
                                Item { Layout.fillWidth: true }
                            }

                            // Hero
                            Text {
                                text: board.sel !== null
                                      ? "Buy " + board.sel.lezAmount + " LEZ"
                                      : ""
                                color: Theme.textPrimary
                                font.pixelSize: Theme.fontTitle
                                font.bold: true
                            }
                            Text {
                                text: board.sel !== null
                                      ? "for " + board.weiToEth(board.sel.ethAmountWei)
                                      : ""
                                color: Theme.textSecondary
                                font.pixelSize: Theme.fontLarge
                            }
                            Text {
                                text: board.sel !== null
                                      ? "1 ETH ≈ " + board.fmtRate(board.rateOf(board.sel)) + " LEZ"
                                      : ""
                                color: {
                                    if (board.sel === null) return Theme.textMuted
                                    var best = offersModel.count > 1 && board.bestRate > 0
                                        && Math.abs(board.rateOf(board.sel) - board.bestRate) < 1e-9
                                    return best ? Theme.success : Theme.textMuted
                                }
                                font.pixelSize: Theme.fontSmall
                                font.family: Theme.monoFont
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                Layout.topMargin: Theme.spacingSmall
                                Layout.bottomMargin: Theme.spacingSmall
                                height: 1
                                color: Theme.border
                            }

                            // Timelocks
                            RowLayout {
                                Layout.fillWidth: true
                                Text {
                                    text: "LEZ timelock"
                                    color: Theme.textSecondary
                                    font.pixelSize: Theme.fontSmall
                                }
                                Item { Layout.fillWidth: true }
                                Text {
                                    text: {
                                        void board.tick
                                        return board.sel !== null
                                               ? board.fmtRemaining(board.sel.lezTimelock
                                                     - Math.floor(Date.now() / 1000))
                                               : ""
                                    }
                                    color: {
                                        void board.tick
                                        return board.sel !== null
                                               ? board.rampColor(board.sel.lezTimelock
                                                     - Math.floor(Date.now() / 1000))
                                               : Theme.textMuted
                                    }
                                    font.pixelSize: Theme.fontSmall
                                    font.family: Theme.monoFont
                                }
                            }
                            RowLayout {
                                Layout.fillWidth: true
                                Text {
                                    text: "ETH timelock"
                                    color: Theme.textSecondary
                                    font.pixelSize: Theme.fontSmall
                                }
                                Item { Layout.fillWidth: true }
                                Text {
                                    text: {
                                        void board.tick
                                        return board.sel !== null
                                               ? board.fmtRemaining(board.sel.ethTimelock
                                                     - Math.floor(Date.now() / 1000))
                                               : ""
                                    }
                                    color: {
                                        void board.tick
                                        return board.sel !== null
                                               ? board.rampColor(board.sel.ethTimelock
                                                     - Math.floor(Date.now() / 1000))
                                               : Theme.textMuted
                                    }
                                    font.pixelSize: Theme.fontSmall
                                    font.family: Theme.monoFont
                                }
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                Layout.topMargin: Theme.spacingSmall
                                Layout.bottomMargin: Theme.spacingSmall
                                height: 1
                                color: Theme.border
                            }

                            // Identity / verification block
                            DetailField {
                                label: "Maker ETH address"
                                value: board.sel !== null ? board.sel.makerEth : ""
                            }
                            DetailField {
                                label: "Maker LEZ account"
                                value: board.sel !== null ? board.sel.makerLez : ""
                            }
                            DetailField {
                                label: "Hashlock"
                                value: board.sel !== null ? board.sel.hashlock : ""
                            }
                            DetailField {
                                label: "LEZ HTLC program"
                                value: board.sel !== null ? board.sel.lezProgramId : ""
                            }
                            DetailField {
                                label: "ETH HTLC contract"
                                value: board.sel !== null ? board.sel.ethHtlcAddr : ""
                            }

                            // Venue-blocked (ghost) banner: the calm block
                            // explanation shown in place of the accept controls
                            // for a non-canonical offer. The Accept button below
                            // is hidden for these; the accept-time venue check
                            // is the true gate regardless.
                            Rectangle {
                                Layout.fillWidth: true
                                Layout.topMargin: Theme.spacingSmall
                                visible: board.sel !== null && board.sel.blocked
                                implicitHeight: blockedCol.implicitHeight + Theme.spacingNormal * 2
                                radius: Theme.radiusNormal
                                color: Theme.surfaceLight
                                border.color: Theme.warning
                                border.width: 1

                                ColumnLayout {
                                    id: blockedCol
                                    anchors {
                                        left: parent.left
                                        right: parent.right
                                        verticalCenter: parent.verticalCenter
                                        margins: Theme.spacingNormal
                                    }
                                    spacing: 6

                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: 6
                                        Text {
                                            text: "🛡"
                                            font.pixelSize: Theme.fontNormal
                                        }
                                        Text {
                                            text: "Blocked — unsafe"
                                            color: Theme.warning
                                            font.pixelSize: Theme.fontNormal
                                            font.bold: true
                                            textFormat: Text.PlainText
                                        }
                                    }
                                    Text {
                                        Layout.fillWidth: true
                                        textFormat: Text.PlainText
                                        text: "Blocked for your safety — this offer routes through a swap program this app doesn't recognize, so no funds were locked."
                                        color: Theme.textSecondary
                                        font.pixelSize: Theme.fontSmall
                                        wrapMode: Text.Wrap
                                    }
                                }
                            }

                            // Blocked reasons
                            ColumnLayout {
                                Layout.fillWidth: true
                                Layout.topMargin: Theme.spacingSmall
                                spacing: 4
                                visible: !board.canAccept && !board.accepting
                                    && !(board.sel !== null && board.sel.blocked)

                                BlockedReason {
                                    reason: "Backend is starting…"
                                    active: !swapBackend.ready
                                }
                                BlockedReason {
                                    reason: "Waiting for network connection"
                                    active: swapBackend.ready && !swapBackend.messagingConnected
                                }
                                BlockedReason {
                                    reason: "Complete configuration first → Config"
                                    active: swapBackend.ready && !board.configReady
                                    clickTab: board.tabConfig
                                }
                                BlockedReason {
                                    reason: "A swap is already running"
                                    active: swapBackend.running
                                }
                                BlockedReason {
                                    reason: "This offer has expired"
                                    active: board.selExpired
                                }
                            }

                            // Accept error
                            Text {
                                visible: board.acceptError !== ""
                                Layout.fillWidth: true
                                text: board.acceptError
                                color: Theme.error
                                font.pixelSize: Theme.fontSmall
                                wrapMode: Text.Wrap
                            }

                            // ACCEPT — hidden entirely for a ghosted offer (the
                            // block banner above stands in its place).
                            Button {
                                Layout.fillWidth: true
                                Layout.preferredHeight: 48
                                Layout.topMargin: Theme.spacingSmall
                                visible: !(board.sel !== null && board.sel.blocked)
                                enabled: board.canAccept
                                text: board.accepting
                                      ? "Starting swap…"
                                      : (board.sel !== null
                                         ? "Accept — buy " + board.sel.lezAmount + " LEZ"
                                         : "Accept")
                                font.pixelSize: Theme.fontNormal
                                font.bold: true

                                background: Rectangle {
                                    color: parent.enabled
                                           ? (parent.hovered ? Theme.accentHover : Theme.accent)
                                           : Theme.surfaceLight
                                    radius: Theme.radiusNormal
                                }
                                contentItem: Text {
                                    text: parent.text
                                    color: parent.enabled || board.accepting
                                           ? "#ffffff" : Theme.textMuted
                                    horizontalAlignment: Text.AlignHCenter
                                    verticalAlignment: Text.AlignVCenter
                                    font: parent.font
                                }
                                onClicked: board.acceptSelected()
                            }

                            Text {
                                Layout.fillWidth: true
                                visible: !(board.sel !== null && board.sel.blocked)
                                text: "Accepting locks your ETH in the HTLC; the maker then locks LEZ and the swap completes automatically. Progress appears in the Taker tab."
                                color: Theme.textMuted
                                font.pixelSize: 11
                                wrapMode: Text.Wrap
                            }
                        }
                    }
                }
            }
        }
    }

    // --- Inline sub-components -----------------------------------------
    component DetailField: ColumnLayout {
        property string label
        property string value
        spacing: 1
        Layout.fillWidth: true

        Text {
            text: parent.label
            color: Theme.textMuted
            font.pixelSize: 10
            font.letterSpacing: 0.5
        }
        Text {
            Layout.fillWidth: true
            // Offer-derived, attacker-controlled strings render as plain text
            // only — never let a maker inject rich text through an address or
            // program-id field.
            textFormat: Text.PlainText
            text: parent.value !== "" ? parent.value : "—"
            color: Theme.textSecondary
            font.pixelSize: 11
            font.family: Theme.monoFont
            elide: Text.ElideMiddle
        }
    }

    component BlockedReason: RowLayout {
        property string reason
        property bool active: false
        property int clickTab: -1
        visible: active
        spacing: 6

        Text {
            text: "⚠"
            color: Theme.warning
            font.pixelSize: 11
        }
        Text {
            Layout.fillWidth: true
            text: parent.reason
            color: Theme.warning
            font.pixelSize: 12
            wrapMode: Text.Wrap

            MouseArea {
                anchors.fill: parent
                enabled: parent.parent.clickTab >= 0
                cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                onClicked: tabBar.currentIndex = parent.parent.clickTab
            }
        }
    }
}
