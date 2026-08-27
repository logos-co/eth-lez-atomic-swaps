import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme
import SwapFormat
import SwapCopy

// The app shell: navigation, the account strip, and the content stack.
//
// Navigation is grouped rather than flat. Seven tabs of equal weight gave the
// two personas crammed into them — the taker (most users) and the maker
// (advanced) — no front door. Everyday work is on the left; the maker and
// recovery surfaces sit after a divider on the right.
//
// Tab identity is now by NAME, not index. Views emit navigation intents
// (OfferBoard's navigateToSetup/Sell/Swap, SetupView's finished) and this file
// resolves them through indexOf, so reordering or renaming a tab can no longer
// silently send a button to the wrong screen.
Item {
    id: root

    // Order here is the order on screen; `primaryCount` is where the divider
    // goes. Add or reorder freely — nothing indexes into this by number.
    readonly property var tabs: [
        { key: "market",  label: "Market"  },
        { key: "swap",    label: "Swap"    },
        { key: "history", label: "History" },
        { key: "sell",    label: "Sell"    },
        { key: "refund",  label: "Refund"  },
        { key: "setup",   label: "Setup"   },
    ]
    readonly property int primaryCount: 3

    function indexOfTab(key) {
        for (var i = 0; i < root.tabs.length; i++)
            if (root.tabs[i].key === key) return i
        return 0
    }
    function goTo(key) {
        tabBar.currentIndex = root.indexOfTab(key)
    }

    // First-run guided setup: once config validation has settled (see the Timer
    // pair below — same pattern as OfferBoard.qml's own readiness check), jump
    // to Setup if the config actually turns out incomplete. Gated to fire at
    // most once and only while the user is still sitting on the default Market
    // tab, so it can never yank navigation away from someone who has already
    // started using the app.
    property bool validationRequested: false
    property bool validationSettled: false
    property bool autoNavigatedToSetup: false

    readonly property var validationErrors: {
        try { return JSON.parse(swapBackend.validationErrorsJson || "{}") }
        catch (e) { return {} }
    }
    readonly property int configIssueCount: Object.keys(validationErrors).length
    readonly property bool configReady: validationSettled && configIssueCount === 0

    onValidationSettledChanged: {
        if (validationSettled && !configReady && !autoNavigatedToSetup
                && tabBar.currentIndex === root.indexOfTab("market")) {
            autoNavigatedToSetup = true
            root.goTo("setup")
        }
    }

    Timer {
        interval: 400
        running: swapBackend.ready && !root.validationRequested
        repeat: false
        onTriggered: {
            root.validationRequested = true
            swapBackend.validateConfig()
            validationSettleFallback.start()
        }
    }

    // Settles validation when the answer is "no errors": equal-value writes to
    // validationErrorsJson emit no change signal, so a valid (complete) config
    // would otherwise never flip validationSettled and the guided Setup tab
    // would never get a chance to NOT show up for it.
    Timer {
        id: validationSettleFallback
        interval: 1500
        repeat: false
        onTriggered: root.validationSettled = true
    }

    Connections {
        target: swapBackend
        function onValidationErrorsJsonChanged() {
            root.validationSettled = true
        }
    }

    // --- Balance liveness (issue #57) ------------------------------------
    // The account strip has no Refresh button, so its balances have to be right
    // on their own.
    //
    // The backend already tries: five completion paths call beginBalanceSettle(),
    // which re-arms requestAutomaticBalanceRefresh() up to eight times. But that
    // function only ever calls fetchBalancesFromLoadedEnv(), gated on
    // `!m_loadedEnvPath.isEmpty()` — a path set in exactly one place, a
    // successful loadEnvFile(). Anyone who configured through Setup (i.e. every
    // real user) has an empty m_loadedEnvPath, so every automatic refresh takes
    // the Decision::Unavailable branch and no-ops with only a trace line. That
    // is why the seller's "Available" never moved after a sale.
    //
    // Driving fetchBalances() — the config-backed route — from here fixes it for
    // the config-backed majority without touching the C++ contract or the
    // env-backed path.
    property int balanceSettleTicks: 0

    function refreshBalancesSoon() {
        if (!swapBackend.ready) return
        swapBackend.fetchBalances()
        // A chain write is not visible the instant a job reports done, so keep
        // asking for a while rather than trusting a single read.
        root.balanceSettleTicks = 6
    }

    Timer {
        interval: 4000
        repeat: true
        running: root.balanceSettleTicks > 0
        onTriggered: {
            root.balanceSettleTicks -= 1
            if (!swapBackend.balancesLoading)
                swapBackend.fetchBalances()
        }
    }

    // First read, once there is a config worth reading with.
    //
    // The latch is load-bearing. `running` is a live binding, and
    // applyBalancesResult() leaves ethBalance untouched when a fetch FAILS
    // (swap_ui_plugin.cpp:1656) — so without it, a dead or rate-limited RPC
    // gives: fetch fails -> ethBalance still "" -> binding re-evaluates true ->
    // timer restarts -> fetch again, ~1.4x/second for as long as the app is
    // open. The OfferBoard timer this replaced had the same latch; dropping it
    // turned a one-shot into a hot loop.
    // Bounded retry rather than a one-shot latch. Latching on the ATTEMPT was
    // the obvious fix for the hot loop, but it trades a loud bug for a silent
    // one: if that single read fails (dead RPC, timeout) the strip shows "—" for
    // the rest of the session, because nothing else calls fetchBalances() until
    // a swap or refund happens to finish. Counting attempts stops the loop and
    // still gives a flaky endpoint a few chances.
    property int initialBalanceAttempts: 0
    readonly property int maxInitialBalanceAttempts: 4

    Timer {
        // Back off between tries so a failing endpoint is not hammered.
        interval: 600 + root.initialBalanceAttempts * 3000
        repeat: false
        running: root.initialBalanceAttempts < root.maxInitialBalanceAttempts
                 && swapBackend.ready && root.configReady
                 && swapBackend.ethBalance === "" && !swapBackend.balancesLoading
        onTriggered: {
            root.initialBalanceAttempts += 1
            swapBackend.fetchBalances()
        }
    }

    // A config edit is a new chance: the previous failures may well have been
    // the old RPC URL. Resets the budget without reintroducing the loop, since
    // this only fires on an actual config change, not on every failed read.
    Connections {
        target: swapBackend
        function onEthRpcUrlChanged() { root.initialBalanceAttempts = 0 }
        function onLezSequencerUrlChanged() { root.initialBalanceAttempts = 0 }
    }

    // --- Sticky error notice ---------------------------------------------
    // The strip cannot bind straight to swapBackend.errorMessage, because
    // fetchBalances() opens with setErrorMessage("") (swap_ui_plugin.cpp:1697)
    // and the settle loop above calls it every 4s for 24s after any run ends.
    // A failed swap sets takerRunning=false (arming the settle window) and THEN
    // writes the failure into errorMessage — so the message the user most needs
    // would appear and then be silently erased four seconds later.
    //
    // Latching it keeps the failure on screen until the user dismisses it or a
    // new run starts. Errors about money should not evaporate on a timer.
    property string noticeError: ""

    Connections {
        target: swapBackend
        function onErrorMessageChanged() {
            if (swapBackend.errorMessage !== "")
                root.noticeError = swapBackend.errorMessage
        }
        // A new attempt supersedes the last failure.
        function onRunningChanged() {
            if (swapBackend.running) root.noticeError = ""
        }
        // Refunds too. refundLez/refundEth do set makerRunning/takerRunning (so
        // `running` covers them), but this view should not depend on that
        // internal detail — the Refund page is precisely where someone goes
        // after a failure, and leaving the old error stacked above the new
        // action reads as "the refund failed as well".
        function onRefundsLoadingChanged() {
            if (swapBackend.refundsLoading) root.noticeError = ""
        }
    }

    // The same failure, twice, on one screen. handleTakerFinished() writes
    // takerResultJson (the "Your swap" receipt card) and then calls
    // setResultStatus(), which writes errorMessage (this strip) — one backend
    // event, two surfaces. On 0.4.4 that put the raw JSON-RPC insufficient-funds
    // text on screen in both places; #157 translated both into the same plain
    // sentence, which still reads as two separate problems ~200px apart.
    //
    // The receipt card is the better home for a failure that finished a run:
    // it is attached to the swap that failed, HistoryView re-renders the
    // identical card for the archived copy, and an error receipt has nothing
    // else in it (every evidence row is hidden behind !isError) — take the
    // sentence out and it is a bare "ERROR" box.
    //
    // So the GLOBAL notice stands down while the tab that owns that receipt is
    // the one being read, and returns the moment the user navigates away —
    // which is precisely when a global notice earns its place, since the copy
    // sends them to the Setup tab. Nothing else changes: noticeError still
    // latches, dismissal still works, and every failure that produces no
    // receipt (start failures, config, balances, refunds) is untouched.
    //
    // Both helpers are PURE — everything they need is an argument — so
    // tests/insufficient-eth-guard.test.mjs evaluates them straight out of this
    // file rather than restating the rule in JavaScript.
    function resultError(resultJson) {
        if (!resultJson) return ""
        // Mirrors ReceiptCard's own parse, including its fallback of treating
        // unparseable JSON as the error text itself.
        try {
            var parsed = JSON.parse(resultJson)
            return parsed && parsed.error !== undefined ? String(parsed.error) : ""
        } catch (e) {
            return resultJson
        }
    }

    function noticeDuplicatesReceipt(noticeText, currentTabIndex, receiptTabIndex,
                                     receiptHeadline) {
        if (noticeText === "") return false
        if (currentTabIndex !== receiptTabIndex) return false
        return receiptHeadline !== "" && receiptHeadline === noticeText
    }

    // Only the taker receipt can duplicate: MakerView's ReceiptCard is built
    // from loopReceipt (completed sales only) and RefundView's ResultCard shows
    // the raw error, not this sentence.
    readonly property bool noticeOnReceipt:
        root.noticeDuplicatesReceipt(
            root.noticeError,
            tabBar.currentIndex,
            root.indexOfTab("swap"),
            Copy.friendlyError(root.resultError(swapBackend.takerResultJson)))

    Connections {
        target: swapBackend
        // A completed sale in the selling loop — the case #57 reported.
        function onAutoAcceptCompletedChanged() { root.refreshBalancesSoon() }
        // Any run finishing, in either direction, moves both balances.
        function onTakerRunningChanged() {
            if (!swapBackend.takerRunning) root.refreshBalancesSoon()
        }
        function onMakerRunningChanged() {
            if (!swapBackend.makerRunning) root.refreshBalancesSoon()
        }
        function onAutoAcceptRunningChanged() {
            if (!swapBackend.autoAcceptRunning) root.refreshBalancesSoon()
        }
        // A refund returns funds too.
        function onRefundsLoadingChanged() {
            if (!swapBackend.refundsLoading) root.refreshBalancesSoon()
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // --- Navigation --------------------------------------------------
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 44
            color: Theme.surface

            TabBar {
                id: tabBar
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                background: null

                Repeater {
                    model: root.tabs
                    TabButton {
                        required property int index
                        required property var modelData

                        text: modelData.label
                        width: implicitWidth
                        leftPadding: 20
                        rightPadding: 20
                        font.pixelSize: Theme.fontNormal
                        // Accessibility: a bare TabButton surfaces to macOS as an
                        // AXRadioButton but advertises no AXPress action, so
                        // assistive tech and UI automation can't switch tabs
                        // (issue #59). Qt's PageTab role has the same limitation on
                        // macOS, so use its radio-button fallback there: Cocoa maps
                        // AXPress on that role to toggleAction. Keep PageTab on
                        // other platforms so the tabs retain native tab-list
                        // membership and semantics.
                        Accessible.role: Qt.platform.os === "osx"
                                         ? Accessible.RadioButton
                                         : Accessible.PageTab
                        Accessible.name: modelData.label
                        Accessible.checkable: Qt.platform.os === "osx"
                        Accessible.checked: Qt.platform.os === "osx"
                                            && tabBar.currentIndex === index
                        Accessible.selectable: Qt.platform.os !== "osx"
                        Accessible.selected: Qt.platform.os !== "osx"
                                             && tabBar.currentIndex === index
                        Accessible.onPressAction: tabBar.currentIndex = index
                        Accessible.onToggleAction: tabBar.currentIndex = index

                        contentItem: Text {
                            text: parent.text
                            font: parent.font
                            color: tabBar.currentIndex === index
                                   ? Theme.accent
                                   : (parent.hovered ? Theme.textPrimary : Theme.textSecondary)
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                        }
                        background: Item {
                            // Divider between the everyday tabs and the
                            // advanced ones, drawn on the leading edge of the
                            // first advanced tab.
                            Rectangle {
                                visible: index === root.primaryCount
                                anchors.left: parent.left
                                anchors.verticalCenter: parent.verticalCenter
                                width: 1
                                height: 18
                                color: Theme.border
                            }
                            Rectangle {
                                anchors.bottom: parent.bottom
                                width: parent.width
                                height: 2
                                color: tabBar.currentIndex === index
                                       ? Theme.accent : "transparent"
                            }
                        }
                    }
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 1
            color: Theme.border
        }

        // --- Account strip ------------------------------------------------
        // Role badge and balances used to be two stacked bands, with the
        // balances crammed left and a Refresh button stranded at the far right.
        // One line now: who you are, what you hold, and whether you're
        // connected. Balances update themselves; there is no Refresh.
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 52
            color: Theme.surface

            RowLayout {
                anchors {
                    fill: parent
                    leftMargin: Theme.spacingLarge
                    rightMargin: Theme.spacingLarge
                }
                spacing: Theme.spacingLarge

                StatusChip {
                    visible: swapBackend.swapRole === "maker" || swapBackend.swapRole === "taker"
                    text: swapBackend.swapRole === "maker" ? "Selling" : "Buying"
                    status: "waiting"
                    Layout.alignment: Qt.AlignVCenter
                }

                RowLayout {
                    spacing: Theme.spacingSmall
                    Layout.alignment: Qt.AlignVCenter

                    Text {
                        text: "ETH"
                        color: Theme.textMuted
                        font.pixelSize: Theme.fontCaption
                        font.bold: true
                    }
                    Text {
                        text: swapBackend.ethAddress
                              ? Format.shortHex(swapBackend.ethAddress, 6, 4) : "—"
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontCaption
                        font.family: Theme.monoFont
                    }
                    Text {
                        text: swapBackend.ethBalance
                              ? Format.weiToEth(swapBackend.ethBalance)
                              : (swapBackend.balancesLoading ? "Reading…" : "—")
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontNormal
                        font.bold: true
                    }
                }

                Rectangle {
                    Layout.preferredWidth: 1
                    Layout.preferredHeight: 20
                    Layout.alignment: Qt.AlignVCenter
                    color: Theme.border
                }

                RowLayout {
                    spacing: Theme.spacingSmall
                    Layout.alignment: Qt.AlignVCenter

                    Text {
                        text: "LEZ"
                        color: Theme.textMuted
                        font.pixelSize: Theme.fontCaption
                        font.bold: true
                    }
                    Text {
                        text: swapBackend.lezAccount
                              ? Format.shortHex(swapBackend.lezAccount, 6, 4) : "—"
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontCaption
                        font.family: Theme.monoFont
                    }
                    Text {
                        text: swapBackend.lezBalance
                              ? swapBackend.lezBalance + " LEZ"
                              : (swapBackend.balancesLoading ? "Reading…" : "—")
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontNormal
                        font.bold: true
                    }
                }

                // Last thing the engine said. The backend calls setStatus() in
                // ~60 places and most are confirmations, not failures — "App
                // data reset to defaults", "Config loaded from env", "Balances
                // fetched". Deleting the bottom status bar took all of them off
                // screen, so a destructive action like Reset app data completed
                // with no acknowledgement at all. Failures additionally set
                // errorMessage and get the loud strip below; this is the quiet
                // channel for everything else.
                Text {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignVCenter
                    Layout.leftMargin: Theme.spacingSmall
                    text: swapBackend.status
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontCaption
                    elide: Text.ElideRight
                    horizontalAlignment: Text.AlignRight
                }

                // Connection state — one dot, one phrase, same vocabulary as
                // everywhere else in the app.
                StatusChip {
                    id: connectionChip
                    Layout.alignment: Qt.AlignVCenter

                    // A KNOWN zero-peer reading while connected is fleet
                    // isolation: the local node is up and subscribed but nothing
                    // can reach it, which a bare "connected" used to mask (live
                    // 0.4.1 incident: stale delivery_module, empty board, no
                    // error anywhere). Gated on messagingHint so the alarm
                    // respects the backend's post-start grace window — 0 peers
                    // seconds after start is normal dialing.
                    readonly property bool fleetIsolated:
                        swapBackend.messagingConnected
                        && swapBackend.messagingPeerCountKnown
                        && swapBackend.messagingPeerCount === 0
                        && swapBackend.messagingHint !== ""

                    status: {
                        if (connectionChip.fleetIsolated || swapBackend.messagingHint !== "")
                            return "attention"
                        if (swapBackend.messagingConnected) return "live"
                        if (swapBackend.messagingLoading || swapBackend.messagingRetrying)
                            return "working"
                        return "attention"
                    }
                    text: {
                        if (swapBackend.messagingConnected) {
                            if (connectionChip.fleetIsolated) return "No peers"
                            if (swapBackend.messagingHint !== "") return "Needs attention"
                            if (swapBackend.messagingPeerCount > 0)
                                return "Connected · " + swapBackend.messagingPeerCount + " peer"
                                       + (swapBackend.messagingPeerCount !== 1 ? "s" : "")
                            return "Connected"
                        }
                        if (swapBackend.messagingLoading) return "Connecting…"
                        if (swapBackend.messagingRetrying) return "Reconnecting…"
                        if (!swapBackend.ready) return "Starting…"
                        return "Not connected"
                    }
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 1
            color: Theme.border
        }

        // --- Error strip ----------------------------------------------------
        // Errors used to live in the bottom status bar as one line of coloured
        // text, 900px below whatever the user was looking at. A failure that
        // involves money deserves to be in the reading path, so it sits under
        // the account strip and pushes content down rather than whispering.
        Rectangle {
            id: errorStrip
            Layout.fillWidth: true
            visible: root.noticeError !== "" && !root.noticeOnReceipt
            implicitHeight: visible ? errorRow.implicitHeight + Theme.spacingNormal : 0
            color: Qt.rgba(Theme.toneProblem.r, Theme.toneProblem.g,
                           Theme.toneProblem.b, 0.12)

            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width
                height: 1
                color: Theme.toneProblem
                opacity: 0.4
            }

            RowLayout {
                id: errorRow
                anchors {
                    left: parent.left
                    right: parent.right
                    verticalCenter: parent.verticalCenter
                    leftMargin: Theme.spacingLarge
                    rightMargin: Theme.spacingLarge
                }
                spacing: Theme.spacingSmall

                StatusDot {
                    status: "problem"
                    size: 6
                    Layout.alignment: Qt.AlignVCenter
                }
                Text {
                    Layout.fillWidth: true
                    text: root.noticeError
                    color: Theme.toneProblem
                    font.pixelSize: Theme.fontSmall
                    horizontalAlignment: Text.AlignLeft
                    wrapMode: Text.WordWrap
                }
                // Latched, so it needs a way out that isn't "wait and hope".
                Button {
                    Layout.preferredWidth: 24
                    Layout.preferredHeight: 24
                    Layout.alignment: Qt.AlignVCenter
                    flat: true
                    Accessible.role: Accessible.Button
                    Accessible.name: "Dismiss this message"
                    background: Rectangle {
                        color: parent.hovered ? Theme.surfaceLight : "transparent"
                        radius: Theme.radiusSmall
                    }
                    contentItem: Text {
                        text: "✕"
                        color: Theme.toneProblem
                        font.pixelSize: Theme.fontCaption
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }
                    onClicked: root.noticeError = ""
                }
            }
        }

        // --- Content -------------------------------------------------------
        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabBar.currentIndex

            OfferBoard {
                onNavigateToSetup: root.goTo("setup")
                onNavigateToSell: root.goTo("sell")
                onNavigateToSwap: root.goTo("swap")
                onSwapEngaged: (offer) => {
                    takerView.acceptedOffer = offer
                    takerView.swapCompleted = false
                }
                onSwapAbandoned: takerView.acceptedOffer = null
            }
            TakerView {
                id: takerView
                onBrowseMarket: root.goTo("market")
            }
            HistoryView {}
            MakerView {}
            RefundView {}
            SetupView {
                onFinished: root.goTo("market")
            }
        }
    }
}
