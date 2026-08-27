import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme
import SwapFormat

// The single place you set this app up.
//
// It used to be one of two: a guided "Setup" tab and a separate "Config" tab of
// fifteen raw fields, with nothing telling you which one you wanted. Config is
// now this page's "Advanced settings" body (see ConfigForm.qml) — the guided
// path is the answer, and the raw fields stay one click away for someone who
// already has keys.
//
// Four steps, and the fourth is not optional: the app can fund LEZ but CANNOT
// fund Sepolia gas, so a fresh account with LEZ and no ETH still fails its
// first swap on "insufficient funds for gas". Readiness is gated on both sides
// (see isReady) and nothing here says "ready" until both have landed.
//
// Shown first on a fresh install (AtomicSwapView jumps here once config
// validation settles and the config turns out incomplete); reachable any time
// afterwards via its own tab.
PageScaffold {
    id: setupRoot

    title: "Get set up"
    subtitle: "Four steps, then you're trading. No keys to type."

    // Emitted when the user is done and wants to head to the market. The host
    // (AtomicSwapView) owns tab navigation, so this view only signals intent
    // rather than reaching into the tab bar itself.
    signal finished()

    readonly property bool hasEthKey: swapBackend.ethPrivateKey !== ""
    readonly property bool hasLezAccount: swapBackend.lezSigningKey !== ""
    // LEZ side holds funds. lezBalance is a plain-LEZ string from the shared
    // balance refresh (same idiom as hasEthBalance below); any positive value
    // means the account is funded, regardless of whether a funding job ran
    // THIS session.
    readonly property bool hasLezBalance: {
        var n = Number(swapBackend.lezBalance)
        return !isNaN(n) && n > 0
    }
    // LEZ side funded (step 3) — true either because a funding job completed
    // this session, or because the account already holds LEZ (e.g. a relaunch
    // after funding in a previous session). Without the balance half of this,
    // an already-funded user saw "Add funds" and a non-"done" step 3 until
    // they clicked through a job again. NOT the same as "ready" — the
    // Ethereum side still needs gas (see hasEthBalance/isReady).
    readonly property bool lezFunded: swapBackend.setupStep === "Done" || setupRoot.hasLezBalance
    // Ethereum side holds gas. ethBalance is a wei string from the shared
    // balance refresh; any positive value means the user has funded Sepolia.
    readonly property bool hasEthBalance: {
        var n = Number(swapBackend.ethBalance)
        return !isNaN(n) && n > 0
    }
    // Setup is only truly ready when BOTH sides can transact. The app funds LEZ
    // (150 via the faucet) but cannot auto-fund Sepolia ETH — there is no free
    // programmatic faucet — so a fresh account with 150 LEZ and 0 Sepolia ETH
    // still fails its first swap on "insufficient funds for gas" (every 0.4.3
    // tester hit this). Gate the final step and every "ready" claim on both.
    readonly property bool isReady: lezFunded && hasEthBalance
    readonly property bool anyRunning: swapBackend.makerRunning || swapBackend.takerRunning
                                       || swapBackend.autoAcceptRunning

    // A reliable, no-wallet-login-friendly Sepolia faucet. Kept as a COPYABLE
    // string, never a clickable open: Basecamp silently no-ops module-owned
    // external navigation (#84), so an "open" would do nothing — the same
    // reason HexValue's link control copies rather than opens.
    readonly property string sepoliaFaucetUrl: "https://sepolia-faucet.pk910.de/"

    // Which value was last copied, for transient "copied" button feedback.
    // Only the faucet URL needs this now: HexValue owns copy feedback for the
    // addresses.
    property string copiedKind: ""

    // Bounded auto-poll for ETH arrival while the test-ETH step is active (see
    // the ethBalancePoll Timer): count of polls issued, capped so a user who
    // wanders off doesn't have the app hammer the RPC forever.
    property int ethPollCount: 0
    readonly property int ethPollMax: 40

    function humanSetupStep(step) {
        var map = {
            "Initializing":       "Setting up your account on the network…",
            "AlreadyInitialized": "Account already set up",
            "Initialized":        "Account set up on the network",
            "CheckingBalance":    "Solving the faucet's puzzle…",
            "ClaimSubmitted":     "Asked for coins — waiting for the network to confirm…",
            "Claimed":            "Coins confirmed",
            "ClaimFailed":        "That attempt didn't land — trying again…",
            "TargetReached":      "Got enough",
            // Honest: step 3 funds LEZ only. Never say "and ready" here — the
            // test-ETH step below decides that.
            "Done":               "LEZ funded",
        }
        return map[step] || step
    }

    // Elapsed-seconds ticker for the funding status line. Individual phases (an
    // Initialize commit, a claim commit) can each take a minute or more of real
    // chain time with no new event in between — without a moving number the app
    // looks frozen (0.4.1 feedback: "Checking account initialization…" sat still
    // for ~1 minute). Pure QML: ticks while the job runs and resets whenever the
    // backend reports a new step, so it always reads "time spent in the CURRENT
    // phase".
    property int setupStepElapsedSeconds: 0

    // Pure-QML clipboard copy for the faucet URL (an invisible TextEdit whose
    // selection is copied).
    function copyText(value, kind) {
        clipboardHelper.text = value
        clipboardHelper.selectAll()
        clipboardHelper.copy()
        clipboardHelper.text = ""
        setupRoot.copiedKind = kind
        copiedReset.restart()
    }

    // All safe in the default slot: PageScaffold owns the Flickable, so
    // non-visual helpers land inside the content column and cannot steal
    // ScrollView's content slot (#113).
    TextEdit {
        id: clipboardHelper
        visible: false
    }

    Timer {
        id: copiedReset
        interval: 1600
        onTriggered: setupRoot.copiedKind = ""
    }

    Timer {
        running: swapBackend.setupRunning
        interval: 1000
        repeat: true
        onTriggered: setupRoot.setupStepElapsedSeconds += 1
    }

    // Auto-detect Sepolia ETH arrival: poll the shared balance refresh while
    // the user has an ETH key but no ETH yet, so their funding shows up without
    // a manual Refresh. triggeredOnStart gives an immediate first read; the
    // interval is deliberately slow and the run is capped (ethPollMax) so it
    // never hammers the RPC. It stops itself the moment a positive balance
    // lands (hasEthBalance).
    Timer {
        id: ethBalancePoll
        interval: 12000
        repeat: true
        triggeredOnStart: true
        running: swapBackend.ready
                 && setupRoot.hasEthKey
                 && !setupRoot.hasEthBalance
                 && setupRoot.ethPollCount < setupRoot.ethPollMax
        onTriggered: {
            if (swapBackend.balancesLoading)
                return
            setupRoot.ethPollCount += 1
            swapBackend.fetchBalances()
        }
    }

    Connections {
        target: swapBackend
        function onSetupStepChanged() { setupRoot.setupStepElapsedSeconds = 0 }
        function onSetupRunningChanged() { setupRoot.setupStepElapsedSeconds = 0 }
        // A freshly generated key starts from zero ETH — restart the poll
        // budget so arrival on the new address is detected.
        function onEthRecipientAddressChanged() { setupRoot.ethPollCount = 0 }
    }

    // --- Step 1: ETH key ---
    Card {
        tone: setupRoot.hasEthKey ? "done" : "neutral"

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacingSmall
            StatusDot {
                status: setupRoot.hasEthKey ? "live" : "waiting"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            SectionHeader {
                label: "1. Ethereum key"
                detail: setupRoot.hasEthKey ? "done" : ""
                hairline: false
            }
        }

        Text {
            visible: !setupRoot.hasEthKey
            Layout.fillWidth: true
            text: "This is the account your ETH is sent from and refunded to. "
                  + "Generating one keeps it on this machine — nothing is published."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }
        HexValue {
            visible: setupRoot.hasEthKey
            label: "Your address"
            value: swapBackend.ethRecipientAddress
        }
        GhostButton {
            text: setupRoot.hasEthKey ? "Generate a different key" : "Generate a key"
            enabled: !setupRoot.anyRunning
            Layout.preferredHeight: 38
            font.bold: true
            onClicked: swapBackend.setupGenerateEthKey()
        }
    }

    // --- Step 2: LEZ account ---
    Card {
        tone: setupRoot.hasLezAccount ? "done" : "neutral"

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacingSmall
            StatusDot {
                status: setupRoot.hasLezAccount ? "live" : "waiting"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            SectionHeader {
                label: "2. LEZ account"
                detail: setupRoot.hasLezAccount ? "done" : ""
                hairline: false
            }
        }

        Text {
            visible: !setupRoot.hasLezAccount
            Layout.fillWidth: true
            text: "Where your LEZ arrives. Nothing goes on the network until step 3 funds it."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }
        HexValue {
            visible: setupRoot.hasLezAccount
            label: "Your account"
            value: swapBackend.lezAccount !== "" ? swapBackend.lezAccount : ""
        }
        Text {
            visible: setupRoot.hasLezAccount && swapBackend.lezAccount === ""
            Layout.fillWidth: true
            text: "Reading your account…"
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
        }
        GhostButton {
            text: setupRoot.hasLezAccount ? "Generate a different account" : "Create an account"
            enabled: !setupRoot.anyRunning
            Layout.preferredHeight: 38
            font.bold: true
            onClicked: swapBackend.setupGenerateLezAccount()
        }
    }

    // --- Step 3: Fund LEZ ---
    Card {
        tone: setupRoot.lezFunded ? "done"
                                  : (swapBackend.setupRunning ? "active" : "neutral")

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacingSmall
            StatusDot {
                status: setupRoot.lezFunded ? "live"
                                            : (swapBackend.setupRunning ? "working" : "waiting")
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            SectionHeader {
                label: "3. Fund LEZ"
                detail: setupRoot.lezFunded ? "done" : ""
                hairline: false
            }
        }

        Text {
            Layout.fillWidth: true
            text: "Claims free test LEZ, up to "
                  + (swapBackend.setupTarget !== "" ? swapBackend.setupTarget : "150")
                  + ". Each claim solves a small puzzle and then waits for the network, "
                  + "and test-network blocks can be a minute or more apart — the timer "
                  + "below keeps counting while it works."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        PrimaryButton {
            text: swapBackend.setupRunning
                  ? "Adding funds…"
                  : (setupRoot.lezFunded ? "Add more" : "Add funds")
            enabled: setupRoot.hasLezAccount && !swapBackend.setupRunning
                     && !setupRoot.anyRunning
            Layout.fillWidth: true
            Layout.preferredHeight: 42
            onClicked: swapBackend.setupStartFunding()
        }

        RowLayout {
            // Show the confirmation line whenever funding is active, ran this
            // session (setupStep set), OR the account already holds LEZ from a
            // prior session (lezFunded) — otherwise an already-funded relaunch
            // shows a "done" step with no "LEZ funded — N" line beneath it.
            visible: swapBackend.setupRunning || swapBackend.setupStep !== ""
                     || setupRoot.lezFunded
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot {
                status: swapBackend.setupRunning ? "working" : "live"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
                text: {
                    // Already funded from a prior session (no job ran this one):
                    // confirm from the real header balance, mirroring the step's
                    // balance-based "done" state.
                    if (!swapBackend.setupRunning && swapBackend.setupStep === ""
                        && setupRoot.hasLezBalance)
                        return "LEZ funded — " + swapBackend.lezBalance + " LEZ"
                    var parts = [setupRoot.humanSetupStep(swapBackend.setupStep)]
                    // Live per-phase elapsed counter, so a slow chain phase
                    // visibly ticks instead of looking hung.
                    if (swapBackend.setupRunning && setupRoot.setupStepElapsedSeconds > 0)
                        parts[0] += " " + setupRoot.setupStepElapsedSeconds + "s"
                    // The fraction only helps while it reads as progress; once the
                    // target is met "200 / 150" scans as an overflow bug rather than
                    // the success it is. Unparsable values compare false and so keep
                    // the fraction.
                    if (swapBackend.setupBalance !== "")
                        parts.push(Number(swapBackend.setupBalance) >= Number(swapBackend.setupTarget)
                                   ? swapBackend.setupBalance + " LEZ"
                                   : swapBackend.setupBalance + " / " + swapBackend.setupTarget + " LEZ")
                    if (swapBackend.setupClaims > 0)
                        parts.push(swapBackend.setupClaims + " claim" + (swapBackend.setupClaims === 1 ? "" : "s") + " confirmed")
                    return parts.join(" — ")
                }
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }
        }

        // Backend error text, framed rather than dumped raw.
        ColumnLayout {
            visible: swapBackend.setupError !== ""
            Layout.fillWidth: true
            spacing: 4

            Text {
                Layout.fillWidth: true
                text: "That didn't work. Your account is fine — nothing was lost. "
                      + "You can press Add funds to try again."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
            }
            Text {
                Layout.fillWidth: true
                textFormat: Text.PlainText
                text: swapBackend.setupError
                color: Theme.textMuted
                font.pixelSize: Theme.fontCaption
                font.family: Theme.monoFont
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.Wrap
            }
        }
    }

    // --- Step 4: Get test ETH ---
    // The app's faucet funds LEZ only. Ethereum needs gas for every swap and
    // there is no free programmatic Sepolia faucet, so this step guides the
    // user to fund it themselves and auto-detects arrival. Without it a fresh
    // account read "Funded and ready" with 0 Sepolia ETH and the first swap
    // died on "insufficient funds for gas" (every 0.4.3 tester hit this).
    Card {
        tone: setupRoot.hasEthBalance ? "done" : "neutral"

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacingSmall
            StatusDot {
                status: setupRoot.hasEthBalance
                        ? "live"
                        : (setupRoot.hasEthKey ? "working" : "waiting")
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            SectionHeader {
                label: "4. Get test ETH"
                detail: setupRoot.hasEthBalance ? "done" : ""
                hairline: false
            }
        }

        Text {
            Layout.fillWidth: true
            text: "Every swap costs a little Ethereum gas, and the app can't get that "
                  + "for you. Send some Sepolia test-ETH to your address — a few "
                  + "hundredths is plenty, and it's free."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        // The address to fund, with copy built in. Master added a hand-rolled
        // "Copy address" button here to close the standing "Setup ETH address
        // has no copy button" gap; HexValue already carries copy on every row,
        // so the row and the fix arrive together.
        HexValue {
            visible: setupRoot.hasEthKey
            label: "Send it here"
            value: swapBackend.ethRecipientAddress
        }
        Text {
            visible: !setupRoot.hasEthKey
            Layout.fillWidth: true
            text: "Generate an Ethereum key in step 1 first."
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
        }

        // Copy-only, never a clickable open: Basecamp silently no-ops
        // module-owned external navigation (#84). Same idiom as HexValue's
        // explorer links.
        Text {
            Layout.fillWidth: true
            Layout.topMargin: Theme.spacingSmall
            text: "A faucet that gives it away — copy this and paste it in your browser:"
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }
        Text {
            Layout.fillWidth: true
            textFormat: Text.PlainText
            text: setupRoot.sepoliaFaucetUrl
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            font.family: Theme.monoFont
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.Wrap
        }
        GhostButton {
            text: setupRoot.copiedKind === "faucet" ? "Link copied" : "Copy faucet link"
            accented: false
            enabled: !setupRoot.anyRunning
            Layout.preferredHeight: 38
            onClicked: setupRoot.copyText(setupRoot.sepoliaFaucetUrl, "faucet")
        }

        // Live arrival status — auto-polled while this step is active, so
        // funding is detected without a manual refresh.
        RowLayout {
            visible: setupRoot.hasEthKey
            Layout.fillWidth: true
            Layout.topMargin: Theme.spacingSmall
            spacing: Theme.spacingSmall

            StatusDot {
                status: setupRoot.hasEthBalance ? "live" : "working"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
                text: setupRoot.hasEthBalance
                      ? "Arrived — " + Format.weiToEth(swapBackend.ethBalance)
                      : (setupRoot.ethPollCount < setupRoot.ethPollMax
                         ? "Watching for it… you don't need to do anything."
                         : "Still nothing. Press Add funds' refresh, or reopen this tab to look again.")
                color: setupRoot.hasEthBalance ? Theme.toneLive : Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }
        }
    }

    // --- Step 5: Done ---
    // Present but dimmed before it is reachable. It used to be `visible:
    // isReady`, so the destination did not exist until you had already
    // arrived — you could not see there was a final step to aim for.
    Card {
        tone: setupRoot.isReady ? "done" : "neutral"
        opacity: setupRoot.isReady ? 1.0 : 0.45

        Behavior on opacity {
            NumberAnimation { duration: Theme.durNormal }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacingSmall
            StatusDot {
                status: setupRoot.isReady ? "live" : "waiting"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            SectionHeader {
                label: "5. Start trading"
                hairline: false
            }
        }
        Text {
            Layout.fillWidth: true
            text: setupRoot.isReady
                  ? "You're set up — LEZ funded and gas in the tank. Head to the market to browse offers."
                  : "Once both LEZ and test-ETH have landed, this is where you head to the market."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }
        PrimaryButton {
            text: "Go to Market"
            enabled: setupRoot.isReady
            Layout.fillWidth: true
            Layout.preferredHeight: 42
            onClicked: setupRoot.finished()
        }
    }

    // --- Advanced: the former Config tab ---
    Disclosure {
        id: advanced
        label: "Advanced settings"

        // A validation error on a field the guided steps never touch
        // (eth_rpc_url, lez_sequencer_url) would otherwise be invisible: the
        // board's "N setup issues" chip sends you here, and here the errors sit
        // inside a collapsed section with nothing indicating it. Badge it, and
        // open it on arrival when something is actually wrong.
        readonly property int issueCount: {
            try { return Object.keys(JSON.parse(swapBackend.validationErrorsJson || "{}")).length }
            catch (e) { return 0 }
        }
        badge: issueCount > 0
               ? issueCount + (issueCount === 1 ? " needs attention" : " need attention")
               : ""

        // Open it once, when problems first appear — then leave the user alone.
        // setConfigValue() re-runs validateConfig() on every keystroke, so
        // re-opening on any issueCount change meant that fixing one field
        // (2 -> 1) yanked the section back open over a deliberate collapse.
        property bool autoOpened: false
        onIssueCountChanged: {
            if (issueCount > 0 && !advanced.autoOpened) {
                advanced.autoOpened = true
                advanced.expanded = true
            } else if (issueCount === 0) {
                // Problems all resolved: re-arm for a genuinely new batch.
                advanced.autoOpened = false
            }
        }

        Text {
            Layout.fillWidth: true
            text: "Everything the steps above fill in for you, plus swap amounts and "
                  + "timers. Changes save as you type."
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        ConfigForm {}
    }
}
