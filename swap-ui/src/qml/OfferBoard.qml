import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme
import SwapFormat
import SwapCopy
import SwapLinks
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

    // --- Navigation intents ---------------------------------------------
    // This view does not own the tab bar; the shell does. It used to declare
    // its own copies of the tab indices (tabConfig=1/tabMaker=2/tabTaker=3) and
    // assign straight into `tabBar.currentIndex` — an id resolved by dynamic
    // scope from AtomicSwapView. That made the tab ORDER a load-bearing,
    // silently-breakable contract between two files. Emitting intent instead
    // means the shell can reorder, rename or merge tabs freely.
    // Same pattern as SetupView's `finished()`.
    signal navigateToSetup()
    signal navigateToSell()
    signal navigateToSwap()

    // Accepting an offer hands it to the swap screen. Emitted rather than
    // written straight into TakerView's properties (this view used to assign to
    // a `takerView` id resolved by dynamic scope from AtomicSwapView, which
    // broke silently the moment the shell stopped naming that child).
    signal swapEngaged(var offer)
    signal swapAbandoned()
    // Spam caps (defense-in-depth on top of the backend cache's own caps).
    // maxOffers bounds the total rendered set so a spammer can't grow the
    // board unbounded; maxGhostRows bounds the "blocked — unsafe" ghost rows
    // so a flood of non-canonical offers can't bury honest ones (beyond the
    // cap, venue-mismatch offers are silently dropped, not ghosted).
    readonly property int maxOffers: 200
    readonly property int maxGhostRows: 4

    // Offer table column model — single source of truth shared by the
    // header row and every delegate row so they can never drift apart.
    // Each cell additionally pins Layout.minimumWidth to 0 (and
    // maximumWidth to the same value): QtQuick.Layouts otherwise defaults
    // an unset minimumWidth to the item's *implicit content width*, which
    // silently grows a cell past its preferredWidth whenever the content is
    // wider than the column (the bold, letter-spaced header labels vs. the
    // variable-length live amounts) — pushing every following column out of
    // sync between the two rows even though both declare the same number.
    readonly property int colSpacerW: 12
    readonly property int colOfferW: 150
    readonly property int colRateW: 96
    readonly property int colAgeW: 44
    readonly property int colExpiresW: 72

    // Reading-column cap. The detail pane is a fixed 380 and only MAKER is
    // elastic, so past ~1400 the extra width is spent entirely on one column of
    // truncated addresses — capping costs nothing but whitespace.
    //
    // ONE number feeds both the status strip and the table, which is the whole
    // point: capping only the content would leave the strip's margins on the
    // window edge while the table's identical margins moved inward, and the two
    // rows would visibly disagree about where the page starts. The strip's
    // background and hairline stay full-bleed — a rule that stops mid-window
    // reads as a rendering fault, not as a margin.
    readonly property int contentInset:
        Math.max(0, (board.width - Theme.boardMaxWidth) / 2)

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

    // The LEZ explorer only indexes the public testnet, so a board pointed at
    // any other sequencer must not offer a link that resolves nowhere.
    readonly property bool lezExplorerOk:
        Links.isLezTestnet(swapBackend.lezSequencerUrl)

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
        if (marketLive) {
            swapBackend.fetchOffers()
            requestOffersNow()
        }
    }

    // --- RFQ: on-demand offer requests (feat/rfq-on-demand-offers) ---------
    // Publish an anonymous offer-request whenever the Market tab is live, so a
    // maker responds with its current offer immediately instead of the board
    // waiting for the maker's slow fallback heartbeat. Re-published every
    // requestIntervalMs while the tab stays open (covers a maker that comes
    // online after us) and stops when the tab closes. The maker heartbeat is
    // still the reliable baseline — this only accelerates the first fill. The
    // request carries no taker identity (privacy); see offer-publisher/rfq.mjs.
    readonly property int requestIntervalMs: 20000

    function requestOffersNow() {
        if (board.marketLive)
            swapBackend.publishOfferRequest()
    }

    // Re-ping when the tab is re-opened while the market was already live (the
    // marketLive transition above won't fire in that case).
    onVisibleChanged: {
        if (visible)
            requestOffersNow()
    }

    Timer {
        id: requestTimer
        interval: board.requestIntervalMs
        running: board.marketLive && board.visible
        repeat: true
        onTriggered: board.requestOffersNow()
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
            board.selectedKey = board.selectedKey === "" ? firstHonestKey() : ""
        board.modelRev++
    }

    // Newest offer the user could actually accept. Auto-selection deliberately
    // skips ghosts: rows are inserted newest-first, so a single non-canonical
    // offer arriving would otherwise become the board's default selection and
    // open the app on a "Blocked — unsafe" banner with no accept button —
    // making the safety net look like the app's own failure. Returns "" when
    // every row is blocked, which leaves the detail pane on its neutral
    // "Select an offer to inspect" state rather than showcasing the ghost.
    function firstHonestKey() {
        for (var i = 0; i < offersModel.count; i++) {
            var o = offersModel.get(i)
            if (!o.blocked) return o.key
        }
        return ""
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
        board.swapEngaged(offer)
        swapBackend.acceptOfferAndStartTaker(offer)
    }

    // --- Formatting helpers --------------------------------------------
    // Amounts, rates and hex truncation live in SwapFormat.Format — this file's
    // private weiToEth/fmtRate were character-identical copies, and its
    // shortHex was one of nine competing truncation shapes.
    //
    // fmtAge and fmtRemaining below stay LOCAL on purpose; see their comments.
    function rateOf(o) {
        var eth = Number(o.ethAmountWei) / 1e18
        var lez = Number(o.lezAmount)
        if (!(eth > 0) || !(lez > 0)) return 0
        return lez / eth
    }

    // Not Format.timeAgo: that appends " ago", which does not fit the 44px AGE
    // column. This is the bare "3m" tape form.
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

    // Not Format.expiresIn: that takes an ABSOLUTE timestamp at minute
    // granularity, while this takes seconds remaining and has a sub-minute
    // branch — which is what feeds rampColor's urgency in the last minute.
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
                // The reassurance used to be spliced onto the end of this
                // string; it now comes from the SafetyNote under every accept
                // error, so it reads the same however the accept failed.
                board.acceptError = "The swap didn't start. Check you're still "
                    + "connected, then try again."
                board.swapAbandoned()
            }
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
                board.navigateToSwap()
            }
        }

        function onErrorMessageChanged() {
            if (board.accepting && swapBackend.errorMessage !== "") {
                board.accepting = false
                board.acceptError = swapBackend.errorMessage
                board.swapAbandoned()
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
                    leftMargin: Theme.spacingLarge + board.contentInset
                    rightMargin: Theme.spacingLarge + board.contentInset
                }
                spacing: Theme.spacingNormal

                // Live dot
                StatusDot {
                    status: board.marketLive ? "live" : "attention"
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
                        font.pixelSize: Theme.fontCaption
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

                // Setup readiness chip. "Checking…" is deliberately `waiting`
                // (grey) rather than the amber it used to be: a validation
                // round-trip in flight is an ordinary precondition, and amber
                // here is the same treatment as a real problem.
                StatusChip {
                    status: board.configReady
                            ? "live"
                            : (board.validationRequested ? "attention" : "waiting")
                    // Settled setup is a fact, not a heartbeat. `live` pulses
                    // by default, and this chip sits in the same strip as the
                    // LIVE MARKET dot — two green pulses saying different
                    // things is exactly the drift StatusDot exists to end.
                    pulsing: false
                    text: board.configReady
                          ? "Ready to trade"
                          : (board.validationRequested
                             ? board.configIssueCount + " setup issue"
                               + (board.configIssueCount === 1 ? "" : "s")
                             : "Checking setup…")
                    clickable: true
                    onClicked: board.navigateToSetup()
                }

                // Maker CTA. GhostButton's accent-outline treatment was lifted
                // from this exact button — see its doc comment.
                GhostButton {
                    text: "+ Make an offer"
                    Layout.preferredHeight: 24
                    font.pixelSize: Theme.fontCaption
                    font.bold: true
                    onClicked: board.navigateToSell()
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

                EmptyState {
                    anchors.centerIn: parent
                    width: Math.min(parent.width - Theme.spacingXLarge * 2, 440)

                    tone: (board.marketLive && !board.fleetIsolated)
                          ? Theme.accent : Theme.warning

                    title: {
                        if (!swapBackend.ready)
                            return "Starting the swap engine…"
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
                            return "Connected, but nobody else is"
                        if (swapBackend.messagingHint !== "")
                            return "The swap network needs attention"
                        return "No offers on the board yet"
                    }

                    subtitle: {
                        if (!swapBackend.ready)
                            return "This takes a few seconds."
                        if (!board.browseReady)
                            return "Add your ETH RPC and LEZ sequencer details in Setup to browse the market."
                        if (!swapBackend.messagingConnected)
                            return swapBackend.messagingRetrying
                                   ? "Looking for the swap network. Offers stream in as soon as a peer answers."
                                   : "The swap network is starting automatically."
                        if (board.fleetIsolated)
                            return "You're on the swap network but not attached to any peers, so no offers can reach you. This usually clears on its own; if it doesn't, check for a module update in Basecamp and restart it."
                        // The hint is a diagnostic written for whoever runs
                        // the network, not for someone trying to buy LEZ. Say
                        // what it means for them here; the raw text is kept
                        // verbatim below as secondary detail.
                        if (swapBackend.messagingHint !== "")
                            return "You're connected, but the swap network is reporting a problem, so offers may not reach you."
                        // Browsing never required setup; only accepting an
                        // offer does. Nudge the still-unconfigured case toward
                        // setting up to *trade*, not "to see offers" — offers
                        // are already visible at this point.
                        return board.configReady
                               ? "The market is waking up — offers appear here the moment sellers publish them."
                               : "The market is waking up — offers appear here the moment sellers publish them. Finish Setup once you see one you like."
                    }

                    // Raw diagnostic, kept but demoted: it names module
                    // versions and cluster ids, which is the right level of
                    // detail for a bug report and the wrong one for a headline.
                    Text {
                        visible: swapBackend.ready
                                 && swapBackend.messagingConnected
                                 && !board.fleetIsolated
                                 && swapBackend.messagingHint !== ""
                        Layout.fillWidth: true
                        Layout.maximumWidth: 440
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        textFormat: Text.PlainText
                        text: swapBackend.messagingHint
                        color: Theme.textMuted
                        font.pixelSize: Theme.fontCaption
                        font.family: Theme.monoFont
                    }

                    PrimaryButton {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 200
                        Layout.preferredHeight: 42
                        visible: swapBackend.ready
                                 && (board.configReady || board.validationRequested)
                        text: board.configReady ? "Make an offer" : "Finish setup"
                        onClicked: board.configReady
                                   ? board.navigateToSell()
                                   : board.navigateToSetup()
                    }

                    Text {
                        Layout.alignment: Qt.AlignHCenter
                        visible: board.marketLive
                        text: "scanning every " + (board.pollIntervalMs / 1000) + "s"
                        color: Theme.textMuted
                        font.pixelSize: Theme.fontCaption
                    }
                }
            }

            // Master-detail split
            RowLayout {
                anchors {
                    fill: parent
                    leftMargin: board.contentInset
                    rightMargin: board.contentInset
                }
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

                            Item { width: board.colSpacerW }
                            Text {
                                Layout.preferredWidth: board.colOfferW
                                Layout.minimumWidth: 0
                                Layout.maximumWidth: board.colOfferW
                                elide: Text.ElideRight
                                horizontalAlignment: Text.AlignLeft
                                text: "OFFER"
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontCaption; font.bold: true
                                font.letterSpacing: 1
                            }
                            Text {
                                Layout.preferredWidth: board.colRateW
                                Layout.minimumWidth: 0
                                Layout.maximumWidth: board.colRateW
                                elide: Text.ElideRight
                                horizontalAlignment: Text.AlignRight
                                text: "RATE LEZ/ETH"
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontCaption; font.bold: true
                                font.letterSpacing: 1
                            }
                            Text {
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                elide: Text.ElideRight
                                horizontalAlignment: Text.AlignLeft
                                // SELLER, not MAKER: this column holds the same
                                // address the detail pane calls "Seller ETH
                                // address" two hundred pixels to the right.
                                text: "SELLER"
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontCaption; font.bold: true
                                font.letterSpacing: 1
                            }
                            Text {
                                Layout.preferredWidth: board.colAgeW
                                Layout.minimumWidth: 0
                                Layout.maximumWidth: board.colAgeW
                                elide: Text.ElideRight
                                horizontalAlignment: Text.AlignRight
                                text: "AGE"
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontCaption; font.bold: true
                                font.letterSpacing: 1
                            }
                            Text {
                                Layout.preferredWidth: board.colExpiresW
                                Layout.minimumWidth: 0
                                Layout.maximumWidth: board.colExpiresW
                                elide: Text.ElideRight
                                horizontalAlignment: Text.AlignRight
                                text: "EXPIRES"
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontCaption; font.bold: true
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
                                    width: board.colSpacerW; height: 12

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
                                                from: 6; to: 14; duration: Theme.durPing
                                            }
                                            NumberAnimation {
                                                target: pingRing; property: "height"
                                                from: 6; to: 14; duration: Theme.durPing
                                            }
                                            NumberAnimation {
                                                target: pingRing; property: "opacity"
                                                from: 0.9; to: 0; duration: Theme.durPing
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
                                    Layout.preferredWidth: board.colOfferW
                                    Layout.minimumWidth: 0
                                    Layout.maximumWidth: board.colOfferW
                                    spacing: 6

                                    Text {
                                        textFormat: Text.PlainText
                                        Layout.minimumWidth: 0
                                        elide: Text.ElideRight
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
                                        Layout.minimumWidth: 0
                                        text: Format.weiToEth(model.ethAmountWei)
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
                                    Layout.preferredWidth: board.colRateW
                                    Layout.minimumWidth: 0
                                    Layout.maximumWidth: board.colRateW
                                    elide: Text.ElideRight
                                    horizontalAlignment: Text.AlignRight
                                    textFormat: Text.PlainText
                                    // No ⚠ glyph: amber + bold on a row that is
                                    // already ghosted says it, and the strip's
                                    // "· N blocked" counter says it again. A
                                    // third mark in a 96px cell is noise, and
                                    // wrapping this cell in a RowLayout to fit
                                    // a StatusDot would put a second item into
                                    // the shared column-width model (#135).
                                    text: model.blocked
                                          ? "blocked"
                                          : Format.fmtRate(offerRow.rowRate)
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
                                    Layout.minimumWidth: 0
                                    horizontalAlignment: Text.AlignLeft
                                    text: Format.shortHex(model.makerEth, 8, 4)
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.fontDetail
                                    font.family: Theme.monoFont
                                    elide: Text.ElideRight
                                }

                                // Age
                                Text {
                                    Layout.preferredWidth: board.colAgeW
                                    Layout.minimumWidth: 0
                                    Layout.maximumWidth: board.colAgeW
                                    elide: Text.ElideRight
                                    horizontalAlignment: Text.AlignRight
                                    text: {
                                        void board.tick
                                        return board.fmtAge(model.receivedMs)
                                    }
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.fontCaption
                                    font.family: Theme.monoFont
                                }

                                // Expiry countdown
                                Text {
                                    Layout.preferredWidth: board.colExpiresW
                                    Layout.minimumWidth: 0
                                    Layout.maximumWidth: board.colExpiresW
                                    elide: Text.ElideRight
                                    horizontalAlignment: Text.AlignRight
                                    text: board.fmtRemaining(offerRow.remain)
                                    color: board.rampColor(offerRow.remain)
                                    font.pixelSize: Theme.fontCaption
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
                            text: "Offers are advertisements — a swap completes only if the seller is still online."
                            color: Theme.textMuted
                            font.pixelSize: Theme.fontCaption
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
                    // 380, not the old 340: the trust rows are now
                    // label + value + copy + link on ONE line (HexValue), and
                    // at 340 the value column came out ~64px — narrower than a
                    // truncated hash, so every address wrapped to two lines.
                    Layout.preferredWidth: 380
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
                                    font.pixelSize: Theme.fontCaption
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
                                        font.pixelSize: Theme.fontMicro
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
                                      ? "for " + Format.weiToEth(board.sel.ethAmountWei)
                                      : ""
                                color: Theme.textSecondary
                                font.pixelSize: Theme.fontLarge
                            }
                            Text {
                                text: board.sel !== null
                                      ? "1 ETH ≈ " + Format.fmtRate(board.rateOf(board.sel)) + " LEZ"
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

                            // Identity / verification block — the trust
                            // surface. This is the screen where someone
                            // decides whether a counterparty is who they say
                            // before locking ETH, and until now it was the one
                            // place that showed these values with no way to
                            // copy them and no way to look them up.
                            //
                            // Only the LEZ side gets explorer links: SwapLinks
                            // derives an Ethereum explorer from a CHAIN ID, and
                            // the board has no chain id to go on before a swap
                            // starts (takerEthChainId is a fact about a run in
                            // progress). Guessing one from the configured RPC
                            // would be a claim from a config file pointed at a
                            // block explorer — exactly what SwapLinks refuses
                            // to do. Copy still works on every row.
                            OfferField {
                                label: "Seller ETH address"
                                value: board.sel !== null ? board.sel.makerEth : ""
                            }
                            OfferField {
                                label: "Seller LEZ account"
                                value: board.sel !== null ? board.sel.makerLez : ""
                                link: board.lezExplorerOk && board.sel !== null
                                      ? Links.lezAccount(board.sel.makerLez) : ""
                            }
                            // An offer is an advertisement, not a swap: the
                            // hashlock only exists once one actually starts.
                            // Showing a permanent placeholder here would spend
                            // a row of the trust surface on nothing, so the row
                            // appears only for an offer that carries a real one.
                            OfferField {
                                label: "Hashlock"
                                value: board.sel !== null ? board.sel.hashlock : ""
                                visible: board.sel !== null && board.sel.hashlock !== ""
                            }
                            OfferField {
                                label: "LEZ program"
                                value: board.sel !== null ? board.sel.lezProgramId : ""
                                link: board.lezExplorerOk && board.sel !== null
                                      ? Links.lezAccount(board.sel.lezProgramId) : ""
                            }
                            OfferField {
                                label: "ETH contract"
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
                                        spacing: Theme.spacingSmall
                                        // Was a 🛡 emoji: a full-colour glyph
                                        // rendered by the system font, at a
                                        // size and hue nothing else in the app
                                        // uses. The status vocabulary already
                                        // has a mark for "the user should look".
                                        StatusDot {
                                            status: "attention"
                                            Layout.alignment: Qt.AlignVCenter
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
                                        text: "This offer settles through a swap program the app doesn't recognise, so it can't be trusted to release your funds."
                                        color: Theme.textSecondary
                                        font.pixelSize: Theme.fontSmall
                                        wrapMode: Text.Wrap
                                    }
                                    // The reassurance belongs in the app's one
                                    // reassurance motif, not buried as a clause
                                    // at the end of the explanation.
                                    SafetyNote {
                                        Layout.topMargin: 2
                                        text: Copy.nothingLockedYet
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
                                    reason: "Finish setting up first — open Setup"
                                    active: swapBackend.ready && !board.configReady
                                    clickAction: () => board.navigateToSetup()
                                }
                                // A refund also raises makerRunning/takerRunning
                                // (refundLez/refundEth set them alongside
                                // refundsLoading), so it has to be checked
                                // FIRST — otherwise someone mid-refund is told
                                // to "stop your sale" and sent to the Sell tab.
                                BlockedReason {
                                    reason: "Wait for your refund to finish"
                                    active: swapBackend.refundsLoading
                                }
                                // Otherwise route by which side is actually
                                // busy. `swapBackend.running` covers the selling
                                // loop too, so sending everyone to the buyer's
                                // Swap screen dumped a seller on a page with
                                // nothing about their sale on it.
                                BlockedReason {
                                    reason: swapBackend.makerRunning || swapBackend.autoAcceptRunning
                                            ? "Finish or stop your sale first"
                                            : "Finish your swap in progress first"
                                    active: swapBackend.running && !swapBackend.refundsLoading
                                    clickAction: () => {
                                        if (swapBackend.makerRunning || swapBackend.autoAcceptRunning)
                                            board.navigateToSell()
                                        else
                                            board.navigateToSwap()
                                    }
                                }
                                BlockedReason {
                                    reason: "This offer has expired"
                                    active: board.selExpired
                                }
                            }

                            // Accept error. A failed accept is the single most
                            // frightening thing that can happen on this screen
                            // — the user just clicked a button that spends
                            // their ETH and got told it went wrong — and it
                            // was the one error surface with no reassurance on
                            // it at all. Nothing is locked until the swap
                            // actually starts, so say so.
                            ColumnLayout {
                                Layout.fillWidth: true
                                Layout.topMargin: Theme.spacingSmall
                                visible: board.acceptError !== ""
                                spacing: Theme.spacingSmall

                                Text {
                                    Layout.fillWidth: true
                                    // Relayed from swapBackend.errorMessage,
                                    // which can quote chain reverts and offer
                                    // fields — not author-written copy.
                                    textFormat: Text.PlainText
                                    text: board.acceptError
                                    color: Theme.error
                                    font.pixelSize: Theme.fontSmall
                                    wrapMode: Text.Wrap
                                }
                                SafetyNote {
                                    text: Copy.nothingLockedYet
                                }
                            }

                            // ACCEPT — hidden entirely for a ghosted offer (the
                            // block banner above stands in its place).
                            PrimaryButton {
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
                                onClicked: board.acceptSelected()
                            }

                            Text {
                                Layout.fillWidth: true
                                visible: !(board.sel !== null && board.sel.blocked)
                                // Was "the maker then locks LEZ … the Taker
                                // tab": one protocol role the user never sees,
                                // and one tab that no longer exists.
                                text: "Accepting locks your ETH in escrow; the seller then locks their LEZ and the swap finishes on its own. You can watch it on the Swap tab."
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontCaption
                                wrapMode: Text.Wrap
                            }
                        }
                    }
                }
            }
        }
    }

    // --- Inline sub-components -----------------------------------------
    // One trust row on the detail pane. Replaces the private DetailField,
    // which stacked an un-copyable, un-linkable value under its label. The
    // narrower label column (vs HexValue's 128 default) buys back width for
    // the value in a 380px pane; every sibling shares it so the values line up.
    component OfferField: HexValue {
        labelWidth: 104
        reserveLinkSlot: true
    }

    // Why the Accept button is currently unavailable.
    //
    // These are ordinary preconditions — "the backend is still starting", "a
    // swap is already running" — not hazards. They used to render amber with a
    // ⚠, which is the same treatment as a genuine warning; training the eye to
    // ignore amber is exactly how a real alarm gets missed. Steady neutral dot,
    // secondary text. `severity: "attention"` is still available for the cases
    // that have actually gone wrong.
    component BlockedReason: RowLayout {
        id: blocked
        property string reason
        property bool active: false
        property string severity: "waiting"
        // Optional navigation, e.g. () => board.navigateToSetup()
        property var clickAction: null
        visible: active
        spacing: Theme.spacingSmall

        StatusDot {
            status: blocked.severity
            size: 6
            Layout.alignment: Qt.AlignVCenter
        }
        Text {
            Layout.fillWidth: true
            text: blocked.reason
            color: blocked.severity === "waiting"
                   ? Theme.textSecondary : Theme.toneFor(blocked.severity)
            font.pixelSize: Theme.fontDetail
            wrapMode: Text.Wrap

            MouseArea {
                anchors.fill: parent
                enabled: blocked.clickAction !== null
                cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                onClicked: if (blocked.clickAction) blocked.clickAction()
            }
        }
    }
}
