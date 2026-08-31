import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme
import SwapFormat
import SwapLinks
import "SetupSteps.js" as SetupSteps

// The single place you set this app up.
//
// It used to be one of two: a guided "Setup" tab and a separate "Config" tab of
// fifteen raw fields, with nothing telling you which one you wanted. Config is
// now this page's "Advanced settings" body (see ConfigForm.qml) — the guided
// path is the answer, and the raw fields stay one click away for someone who
// already has keys.
//
// The step order, numbering and count all come from SetupSteps.js (one
// ordered list per mode) so the subtitle can never disagree with the cards.
// The test-ETH step is not optional: the app can fund LEZ but CANNOT fund
// Sepolia gas, so a fresh account with LEZ and no ETH still fails its first
// swap on "insufficient funds for gas". Readiness is gated on both sides (see
// isReady) and nothing here says "ready" until both have landed.
//
// The default flow is faucet-less (issue #166): step 3 only ACTIVATES
// (initializes) the LEZ account. A taker needs zero LEZ to swap, because LEZ
// charges no fees — so the way to GET LEZ is to buy it on the market, which is
// what this app is for. Handing a new user 150 faucet LEZ before they have
// traded for any teaches the wrong first move.
//
// The faucet is not gone, it is demoted: it lives on this page as its own
// named, collapsed section (`otherWaysToGetLez` below), because a seller does
// need LEZ inventory before they can offer any. That is a deliberate secondary
// path, not a hidden switch — the user opens a section, they do not set an
// environment variable.
//
// SWAP_UI_LEZ_FAUCET_MODE=on (swapBackend.setupFaucetless == false;
// swap-ui/src/setup_flow.h) is a DEVELOPER override that restores the legacy
// screens for recorded demo scripts and tooling. It is not how a user chooses
// the faucet, and it is not on the path any user walks.
//
// Shown first on a fresh install (AtomicSwapView jumps here once config
// validation settles and the config turns out incomplete); reachable any time
// afterwards via its own tab.
PageScaffold {
    id: setupRoot

    title: "Get set up"
    subtitle: SetupSteps.subtitle(setupRoot.faucetless)

    // Which of the two flows this page runs — true (faucet-less) unless the
    // developer override SWAP_UI_LEZ_FAUCET_MODE=on is set. Fixed for the
    // process lifetime.
    readonly property bool faucetless: swapBackend.setupFaucetless

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
    // What step 3 needs to have delivered before the LEZ side counts as ready:
    // a funded account in the default flow, an INITIALIZED one in the
    // faucet-less flow. Never inferred from a balance in the faucet-less flow —
    // a credit can land on a never-initialized account, which then fails its
    // swap with a silent drop.
    readonly property bool lezReady: setupRoot.faucetless ? swapBackend.setupInitialized
                                                          : setupRoot.lezFunded
    // Ethereum side holds gas. ethBalance is a wei string from the shared
    // balance refresh; any positive value means the user has funded Sepolia.
    readonly property bool hasEthBalance: {
        var n = Number(swapBackend.ethBalance)
        return !isNaN(n) && n > 0
    }
    // A chain the app cannot currently reach (issue #169). The backend only
    // publishes these after a side has failed repeatedly — a single dropped
    // request stays invisible — so when one is set it is worth interrupting
    // the step's normal copy for. Never a readiness signal: an unreachable
    // chain says nothing about whether the account is funded, so isReady below
    // is deliberately untouched by it.
    readonly property bool ethUnreachable: swapBackend.ethBalanceError !== ""
    readonly property bool lezUnreachable: swapBackend.lezBalanceError !== ""

    // Setup is only truly ready when BOTH sides can transact. The app funds LEZ
    // (150 via the faucet) but cannot auto-fund Sepolia ETH — there is no free
    // programmatic faucet — so a fresh account with 150 LEZ and 0 Sepolia ETH
    // still fails its first swap on "insufficient funds for gas" (every 0.4.3
    // tester hit this). Gate the final step and every "ready" claim on both.
    readonly property bool isReady: lezReady && hasEthBalance
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

    // Which affordance started the job currently reflected in the shared
    // setupRunning/setupStep/setupError PROPs: "activate" (step 3) or "claim"
    // (the faucet section below). Both live on the same page in the
    // faucet-less flow and the backend has one set of progress PROPs, so
    // without this a faucet claim would render "Solving the faucet's puzzle…"
    // and its errors inside step 3's card, under a heading that has nothing to
    // do with either.
    // ...or "faucet" (step 4's Get test ETH button, which reports through the
    // same shared PROPs and would otherwise render "Solving the faucet's
    // puzzle…" inside step 3's card — the exact bug #173 fixed for claims).
    property string setupOrigin: ""
    // Everything that means "THIS card's job is running" binds through these,
    // never through the shared setupRunning directly; the button guards keep
    // the bare setupRunning because no setup job may start while any runs.
    readonly property bool activating: swapBackend.setupRunning
                                       && setupRoot.setupOrigin !== "claim"
                                       && setupRoot.setupOrigin !== "faucet"
    readonly property bool claiming: swapBackend.setupRunning && setupRoot.setupOrigin === "claim"
    readonly property bool requestingEth: swapBackend.setupRunning && setupRoot.setupOrigin === "faucet"

    // Bounded auto-poll for ETH arrival while the test-ETH step is active (see
    // the ethBalancePoll Timer): count of polls issued, capped so a user who
    // wanders off doesn't have the app hammer the RPC forever.
    property int ethPollCount: 0
    readonly property int ethPollMax: 40

    function humanSetupStep(step) {
        var map = {
            "Initializing":       "Setting up your account on the network…",
            // Issue #171: the activation call is silent for as long as it
            // takes a block to commit — up to five minutes on this test
            // network. These two say which kind of waiting is going on, so a
            // long wait reads as the chain being slow rather than the app
            // being stuck. Neither claims the transaction landed; only
            // Initialized/AlreadyInitialized do that.
            "AwaitingCommit":     "Waiting for the network to confirm — test-network blocks can be minutes apart.",
            "Verifying":          "Checking whether it landed…",
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
            // Step 4's in-house drip faucet (see the Get test ETH button).
            // One phase covers the whole round trip — the app solves a puzzle,
            // then the faucet waits for its own transaction to be mined before
            // it answers — so the copy names both halves rather than promising
            // a progression that never arrives.
            "FaucetRequesting":   "Solving the faucet's puzzle, then waiting for the transaction…",
            "FaucetDripped":      "Sent",
        }
        return map[step] || step
    }

    // What the step-4 arrival line says, in priority order. PURE — everything
    // it needs is an argument — so tests/balance-notice.test.mjs evaluates it
    // straight out of this file rather than restating the rule in JavaScript.
    //
    // `unreachable` has to outrank the watching copy (issue #169): while the
    // Sepolia RPC is down, "Watching for it… you don't need to do anything."
    // is simply untrue, and being unable to read the chain is the whole reason
    // the arriving test-ETH never shows up. This is the step the user is
    // staring at, so the explanation belongs here rather than in a global
    // banner they never asked for. An arrival that HAS been read still wins
    // over both: a balance on screen is proof the endpoint answered.
    function ethArrivalLine(arrived, unreachable, pollCount, pollMax) {
        if (arrived) return arrived
        if (unreachable) return unreachable
        return pollCount < pollMax
            ? "Watching for it… you don't need to do anything."
            : "Still nothing. Press Add funds' refresh, or reopen this tab to look again."
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
                label: SetupSteps.stepLabel("ethKey", setupRoot.faucetless)
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
                label: SetupSteps.stepLabel("lezAccount", setupRoot.faucetless)
                detail: setupRoot.hasLezAccount ? "done" : ""
                hairline: false
            }
        }

        Text {
            visible: !setupRoot.hasLezAccount
            Layout.fillWidth: true
            text: "Where your LEZ arrives. Nothing goes on the network until step "
                  + SetupSteps.stepNumber(setupRoot.faucetless ? "activateLez" : "fundLez",
                                          setupRoot.faucetless)
                  + (setupRoot.faucetless ? " activates it." : " funds it.")
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

    // --- Step 3 (faucet-less flow only): Activate the LEZ account ---
    // Initialize and nothing else. One free signed transaction, idempotent
    // (an already-set-up account answers "already set up" with no
    // transaction). This is the one step a taker cannot skip: the network
    // silently drops actions against a never-initialized account, so the
    // gate stays on the real outcome (setupInitialized), never on a balance.
    Card {
        visible: setupRoot.faucetless
        tone: swapBackend.setupInitialized ? "done"
                                           : (setupRoot.activating ? "active" : "neutral")

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacingSmall
            StatusDot {
                status: swapBackend.setupInitialized ? "live"
                                                     : (setupRoot.activating ? "working" : "waiting")
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            SectionHeader {
                label: SetupSteps.stepLabel("activateLez", setupRoot.faucetless)
                detail: swapBackend.setupInitialized ? "done" : ""
                hairline: false
            }
        }

        Text {
            Layout.fillWidth: true
            text: "Registers your account on the LEZ network so it can receive the LEZ "
                  + "you buy. It's free and takes one transaction — you don't need any LEZ "
                  + "to trade; you'll get it from the market. Test-network blocks can be a "
                  + "minute or more apart, so the timer below keeps counting while it works."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        // Whether an account is already registered is a fact on the network,
        // and this app does not remember it between launches (see
        // swap_ui.rep's setupInitialized). Rather than guess — guessing wrong
        // means a swap the sequencer silently drops — say plainly what the
        // button costs someone who is already set up: a read, a second, and
        // no transaction. Without this line, anyone who set up under the old
        // faucet flow opens Setup after updating, sees an unfinished step 3,
        // and has nothing telling them it is a formality.
        Text {
            visible: !swapBackend.setupInitialized && !swapBackend.setupRunning
                     && setupRoot.hasLezAccount
            Layout.fillWidth: true
            text: "If you set this up in an earlier session, press it anyway — it checks "
                  + "the network first and confirms in about a second, without sending "
                  + "anything."
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        PrimaryButton {
            text: setupRoot.activating
                  ? "Activating…"
                  : (swapBackend.setupInitialized ? "Check again" : "Activate account")
            enabled: setupRoot.hasLezAccount && !swapBackend.setupRunning
                     && !setupRoot.anyRunning
            Layout.fillWidth: true
            Layout.preferredHeight: 42
            onClicked: {
                setupRoot.setupOrigin = "activate"
                swapBackend.setupInitializeAccount()
            }
        }

        RowLayout {
            visible: setupRoot.activating
                     || (setupRoot.setupOrigin !== "claim" && setupRoot.setupOrigin !== "faucet"
                         && swapBackend.setupStep !== "")
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot {
                status: setupRoot.activating ? "working" : "live"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
                text: {
                    var line = setupRoot.humanSetupStep(swapBackend.setupStep)
                    if (setupRoot.activating && setupRoot.setupStepElapsedSeconds > 0)
                        line += " " + setupRoot.setupStepElapsedSeconds + "s"
                    return line
                }
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }
        }

        // The LEZ side is unreachable (issue #169). This card and the default
        // flow's "Fund LEZ" card are mutually exclusive (one `faucetless`, one
        // `!faucetless`), so exactly one of these two rows can ever render —
        // but BOTH need it. Without it here, a dead sequencer is completely
        // silent during faucet-less Setup: this is the only LEZ step in that
        // flow, and the shell's stale-balance strip deliberately stands down
        // on the Setup tab because Setup is supposed to own the message.
        // Silence is the one outcome #169 rules out. Here it also explains
        // why Activate account is failing, since activation is exactly the
        // thing that needs the sequencer.
        RowLayout {
            visible: setupRoot.lezUnreachable
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot {
                status: "attention"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                text: swapBackend.lezBalanceError
                color: Theme.toneAttention
                font.pixelSize: Theme.fontSmall
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
            }
        }

        ColumnLayout {
            visible: setupRoot.setupOrigin !== "claim" && setupRoot.setupOrigin !== "faucet"
                     && swapBackend.setupError !== ""
            Layout.fillWidth: true
            spacing: 4

            Text {
                Layout.fillWidth: true
                // Deliberately does not say "that didn't work": after issue
                // #171 an activation error can mean "we never found out",
                // and the detail below says which. Both readings end with
                // the same safe next move, so that is what this line says.
                text: "Your account is fine — nothing was lost. Press Activate "
                      + "account to try again; it checks before it sends anything."
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

    // --- Step 3 (legacy SWAP_UI_LEZ_FAUCET_MODE=on flow): Fund LEZ ---
    // Hidden in the default faucet-less flow, where the same claim lives in
    // the "Get test LEZ without trading" section instead (otherWaysToGetLez).
    Card {
        visible: !setupRoot.faucetless
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
                label: SetupSteps.stepLabel("fundLez", setupRoot.faucetless)
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

        // The LEZ side is unreachable (issue #169). This step's "done" mark and
        // the confirmation line above both read from lezBalance, so while the
        // sequencer cannot be reached this step is showing a number it cannot
        // vouch for — say so here, in the step, instead of raising a global
        // banner for a background poll the user never triggered. Amber, not
        // red: nothing has failed, the app just cannot see right now, and this
        // clears itself on the first good read.
        //
        // Twin of the row in the faucet-less "Activate your LEZ account" card
        // above; the two cards are mutually exclusive, so every LEZ step in
        // every flow carries this. Change one, change both.
        RowLayout {
            visible: setupRoot.lezUnreachable
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot {
                status: "attention"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                text: swapBackend.lezBalanceError
                color: Theme.toneAttention
                font.pixelSize: Theme.fontSmall
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
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
                label: SetupSteps.stepLabel("testEth", setupRoot.faucetless)
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

        // --- In-house drip faucet (PoC) ---
        // The whole point of the button: the app already knows the address, so
        // this collapses "copy link -> browser -> mine in a tab -> paste
        // address -> come back" into one press. Hidden entirely when no faucet
        // is configured for the build (ethFaucetUrl empty), leaving the card
        // exactly as 0.4.6 shipped it.
        PrimaryButton {
            visible: swapBackend.ethFaucetUrl !== ""
            text: setupRoot.requestingEth ? "Getting test ETH…" : "Get test ETH"
            // Needs an address to send to, and no other setup job may be
            // running — the backend serializes them through one setupRunning.
            enabled: setupRoot.hasEthKey && !swapBackend.setupRunning && !setupRoot.anyRunning
            Layout.preferredHeight: 38
            onClicked: {
                setupRoot.setupOrigin = "faucet"
                swapBackend.setupRequestTestEth()
            }
        }

        // Live progress for a request started HERE, with the same per-phase
        // elapsed ticker the numbered steps use. The wait is a real
        // proof-of-work solve plus a chain inclusion — tens of seconds — so a
        // moving number is the difference between "working" and "hung".
        RowLayout {
            visible: setupRoot.requestingEth
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot {
                status: "working"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
                text: {
                    var line = setupRoot.humanSetupStep(swapBackend.setupStep)
                    if (setupRoot.setupStepElapsedSeconds > 0)
                        line += " " + setupRoot.setupStepElapsedSeconds + "s"
                    return line
                }
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }
        }

        // The drip that landed. The amount comes from the FAUCET, never from a
        // figure compiled into the app: the service owns its drip size and can
        // change it without an app release.
        ColumnLayout {
            visible: swapBackend.setupFaucetTxHash !== "" && !setupRoot.requestingEth
            Layout.fillWidth: true
            spacing: 4

            Text {
                Layout.fillWidth: true
                text: swapBackend.setupFaucetAmountEth !== ""
                      ? "The faucet sent " + swapBackend.setupFaucetAmountEth
                        + " ETH. It's on-chain already — the line below confirms it arrived."
                      : "The faucet sent your test ETH. It's on-chain already — the line below "
                        + "confirms it arrived."
                color: Theme.toneLive
                font.pixelSize: Theme.fontSmall
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
            }
            HexValue {
                label: "Faucet tx"
                value: swapBackend.setupFaucetTxHash
                // Derived from the chain id the FAUCET reported, so an unknown
                // chain resolves to no link rather than to the wrong network's
                // explorer (SwapLinks/Links.qml's rule).
                link: Links.ethTx(swapBackend.setupFaucetTxHash, swapBackend.setupFaucetChainId)
            }
        }

        // A refused or failed request, in THIS card. The faucet writes its own
        // refusals — only it knows how long a cooldown has left — so its
        // sentence is shown verbatim under a fixed lead-in.
        ColumnLayout {
            visible: setupRoot.setupOrigin === "faucet" && swapBackend.setupError !== ""
            Layout.fillWidth: true
            spacing: 4

            Text {
                Layout.fillWidth: true
                text: "The faucet couldn't send test ETH. Nothing was lost, and the faucet "
                      + "links below still work."
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

        // --- Fallback: external faucets ---
        // Kept whether or not the in-house one is configured. It is a single
        // hot key on one VPS with a daily budget; when it is empty, refusing,
        // or down, this is the path that still works.
        //
        // Copy-only, never a clickable open: Basecamp silently no-ops
        // module-owned external navigation (#84). Same idiom as HexValue's
        // explorer links.
        Text {
            Layout.fillWidth: true
            Layout.topMargin: Theme.spacingSmall
            text: swapBackend.ethFaucetUrl !== ""
                  ? "Faucet busy, empty, or unreachable? Copy this and paste it in your browser:"
                  : "A faucet that gives it away — copy this and paste it in your browser:"
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
                status: setupRoot.hasEthBalance
                        ? "live"
                        : (setupRoot.ethUnreachable ? "attention" : "working")
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
                text: setupRoot.ethArrivalLine(
                          setupRoot.hasEthBalance
                              ? "Arrived — " + Format.weiToEth(swapBackend.ethBalance)
                              : "",
                          swapBackend.ethBalanceError,
                          setupRoot.ethPollCount, setupRoot.ethPollMax)
                color: setupRoot.hasEthBalance
                       ? Theme.toneLive
                       : (setupRoot.ethUnreachable ? Theme.toneAttention : Theme.textSecondary)
                font.pixelSize: Theme.fontSmall
            }
        }
    }

    // --- Done (not a numbered step) ---
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
                // Deliberately unnumbered: SetupSteps.stepsFor() lists only
                // the steps you take, and this card is not one of them — it
                // is where you land once those are done (see the header comment).
                label: SetupSteps.TITLES.trade
                hairline: false
            }
        }
        Text {
            Layout.fillWidth: true
            text: setupRoot.faucetless
                  ? (setupRoot.isReady
                     ? "You're set up — account active and gas in the tank. Head to the market to buy LEZ."
                     : "Once you've activated your LEZ account (step "
                       + SetupSteps.stepNumber("activateLez", setupRoot.faucetless)
                       + ") and test-ETH has landed, this is where you head to the market.")
                  : (setupRoot.isReady
                     ? "You're set up — LEZ funded and gas in the tank. Head to the market to browse offers."
                     : "Once both LEZ and test-ETH have landed, this is where you head to the market.")
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

    // --- The faucet's home in the app (faucet-less flow only) ---
    //
    // A peer of the numbered flow, not a step in it. A buyer never needs this:
    // LEZ charges no fees, so you can buy LEZ with a zero LEZ balance, and the
    // market is where LEZ is supposed to come from (issue #166). Asking every
    // new user to decide about a "faucet" — a word they have no reason to know
    // — before they have even seen the market is exactly what that argument is
    // against.
    //
    // But it cannot be an environment variable either. A SELLER needs LEZ
    // inventory before they can offer any, and that is a real, ordinary thing
    // to want from inside the app. So: collapsed by default (the primary path
    // stays four short cards), sitting in the open where it can be found,
    // labelled for what the user gets rather than for the mechanism, and
    // leading with who does NOT need it so nobody opens it out of worry.
    //
    // Hidden under the legacy SWAP_UI_LEZ_FAUCET_MODE=on flow, where this same
    // claim IS step 3 — one claim affordance per flow, never two.
    Disclosure {
        id: otherWaysToGetLez
        visible: setupRoot.faucetless
        label: "Get test LEZ without trading"

        // Admit in the collapsed header what happened to a claim started
        // here, so collapsing the section is never how you lose track of it.
        // That is the difference between progressive disclosure and simply
        // hiding things — see Disclosure.qml's own note on the badge.
        readonly property string claimState: {
            if (setupRoot.setupOrigin !== "claim") return ""
            if (setupRoot.claiming) return "claiming…"
            if (swapBackend.setupError !== "") return "claim failed"
            if (swapBackend.setupStep === "Done") return "claimed"
            return ""
        }
        badge: claimState
        badgeTone: claimState === "claim failed" ? Theme.toneAttention : Theme.toneLive

        Text {
            Layout.fillWidth: true
            text: "You don't need this to buy LEZ. Buying is the whole point of the app: "
                  + "you pay Sepolia ETH on the Market tab and the LEZ arrives — an empty "
                  + "LEZ balance is fine, because LEZ charges no fees."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }
        Text {
            Layout.fillWidth: true
            text: "You do need it to SELL LEZ. A sell offer has to be backed by LEZ you "
                  + "already hold, and on a test network the only other source is this "
                  + "faucet: it hands out free test LEZ, up to "
                  + (swapBackend.setupTarget !== "" ? swapBackend.setupTarget : "150")
                  + " per run. Each claim solves a small puzzle and then waits for the "
                  + "network, so it can take a few minutes."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        GhostButton {
            text: setupRoot.claiming
                  ? "Claiming…"
                  : "Claim test LEZ"
            // Claiming credits an account, and the network silently drops
            // actions against a never-initialized one — so this needs the same
            // account step 3 activates. The funding job initializes first
            // anyway; requiring the account only keeps the button honest about
            // what it needs.
            enabled: setupRoot.hasLezAccount && !swapBackend.setupRunning
                     && !setupRoot.anyRunning
            Layout.preferredHeight: 38
            onClicked: {
                setupRoot.setupOrigin = "claim"
                swapBackend.setupStartFunding()
            }
        }

        Text {
            visible: !setupRoot.hasLezAccount
            Layout.fillWidth: true
            text: "Finish step " + SetupSteps.stepNumber("lezAccount", setupRoot.faucetless)
                  + " first — a claim needs a LEZ account to arrive in."
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        // Live progress for a claim started HERE, with the same per-phase
        // elapsed ticker the numbered steps use — a slow chain phase has to
        // visibly tick rather than look hung.
        RowLayout {
            visible: setupRoot.claiming
                     || (setupRoot.setupOrigin === "claim" && swapBackend.setupStep !== "")
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot {
                status: setupRoot.claiming ? "working" : "live"
                size: 6
                Layout.alignment: Qt.AlignVCenter
            }
            Text {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
                text: {
                    var parts = [setupRoot.humanSetupStep(swapBackend.setupStep)]
                    if (setupRoot.claiming && setupRoot.setupStepElapsedSeconds > 0)
                        parts[0] += " " + setupRoot.setupStepElapsedSeconds + "s"
                    if (swapBackend.setupBalance !== "")
                        parts.push(Number(swapBackend.setupBalance) >= Number(swapBackend.setupTarget)
                                   ? swapBackend.setupBalance + " LEZ"
                                   : swapBackend.setupBalance + " / " + swapBackend.setupTarget + " LEZ")
                    if (swapBackend.setupClaims > 0)
                        parts.push(swapBackend.setupClaims + " claim"
                                   + (swapBackend.setupClaims === 1 ? "" : "s") + " confirmed")
                    return parts.join(" — ")
                }
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }
        }

        ColumnLayout {
            visible: setupRoot.setupOrigin === "claim" && swapBackend.setupError !== ""
            Layout.fillWidth: true
            spacing: 4

            Text {
                Layout.fillWidth: true
                text: "That claim didn't land. Your account is fine — nothing was lost, "
                      + "and your setup steps above are unaffected. Press Claim test LEZ "
                      + "to try again."
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

        Text {
            visible: setupRoot.hasLezBalance
            Layout.fillWidth: true
            text: "You hold " + swapBackend.lezBalance + " LEZ."
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        // The way back. Progressive disclosure owes the reader an exit, and
        // "none of this changed your steps" is the reassurance someone who
        // opened it by accident actually needs.
        Text {
            Layout.fillWidth: true
            text: "None of this changes the steps above — collapse this section and carry "
                  + "on where you left off."
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
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
