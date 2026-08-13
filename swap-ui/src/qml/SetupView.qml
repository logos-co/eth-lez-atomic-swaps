import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme

// Guided first-run setup: create-or-import an ETH key, create a LEZ
// account, then fund it — replacing the worst part of onboarding (hand-
// typing two private keys and two long account IDs into the Config tab,
// see #87/#91) with buttons. Manual paste still works: every field this
// view fills is a plain Config-tab field underneath, so a user who already
// has keys can just use Config directly and skip this view entirely.
//
// Shown first on a fresh install (AtomicSwapView jumps here once config
// validation settles and the config turns out incomplete); reachable any
// time afterwards via its own tab.
ScrollView {
    id: setupRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    // Emitted when the user is done and wants to head to the market. The
    // host (AtomicSwapView) owns tab navigation, so this view only signals
    // intent rather than reaching into the tab bar itself.
    signal finished()

    readonly property bool hasEthKey: swapBackend.ethPrivateKey !== ""
    readonly property bool hasLezAccount: swapBackend.lezSigningKey !== ""
    // LEZ side funded via the pinata faucet (step 3). This is NOT the same as
    // "ready" — the Ethereum side still needs gas (see hasEthBalance/isReady).
    readonly property bool lezFunded: swapBackend.setupStep === "Done"
    // Ethereum side holds gas. ethBalance is a wei string from the shared
    // balance refresh; any positive value means the user has funded Sepolia.
    readonly property bool hasEthBalance: {
        var n = Number(swapBackend.ethBalance)
        return !isNaN(n) && n > 0
    }
    // Setup is only truly ready when BOTH sides can transact: LEZ funded AND
    // Ethereum funded for gas. The app funds LEZ (150 via the faucet) but
    // CANNOT auto-fund Sepolia ETH — there's no free programmatic faucet — so
    // a fresh account with 150 LEZ and 0 Sepolia ETH still fails its first
    // swap on "insufficient funds for gas" (every 0.4.3 tester hit this).
    // Gate the Done step and every "ready" claim on both.
    readonly property bool isReady: lezFunded && hasEthBalance
    readonly property bool anyRunning: swapBackend.makerRunning || swapBackend.takerRunning || swapBackend.autoAcceptRunning

    // A reliable, no-wallet-login-friendly Sepolia faucet. Kept as a COPYABLE
    // string, never a clickable open: Basecamp silently no-ops module-owned
    // external navigation (#84), so Qt.openUrlExternally would do nothing —
    // the receipts use the same copy-and-paste idiom.
    readonly property string sepoliaFaucetUrl: "https://sepolia-faucet.pk910.de/"

    // Which value was last copied, for transient "copied" button feedback.
    property string copiedKind: ""

    // Bounded auto-poll for ETH arrival while the Get-test-ETH step is active
    // (see the ethBalancePoll Timer): count of polls issued, capped so a user
    // who wanders off doesn't have the app hammer the RPC forever.
    property int ethPollCount: 0
    readonly property int ethPollMax: 40

    function humanSetupStep(step) {
        var map = {
            "Initializing":       "Checking account initialization…",
            "AlreadyInitialized": "Account already initialized",
            "Initialized":        "Account initialized on-chain",
            "CheckingBalance":    "Solving faucet proof-of-work…",
            "ClaimSubmitted":     "Claim submitted — waiting for on-chain commit…",
            "Claimed":            "Claim committed on-chain",
            "ClaimFailed":        "Claim attempt failed, retrying…",
            "TargetReached":      "Target reached",
            // Honest: step 3 funds LEZ only. "Ready" is gated on ETH too, so
            // never say "and ready" here — the Get-test-ETH step below decides.
            "Done":               "LEZ funded",
        }
        return map[step] || step
    }

    // wei string → human ETH/Gwei, matching the app-wide balance formatter
    // (AtomicSwapView.weiToEth / ReceiptCard.ethDisplay).
    function ethBalanceDisplay() {
        var n = Number(swapBackend.ethBalance)
        if (isNaN(n) || n === 0) return "0 ETH"
        var eth = n / 1e18
        if (eth >= 0.001) return eth.toFixed(6).replace(/\.?0+$/, '') + " ETH"
        var gwei = n / 1e9
        if (gwei >= 1) return gwei.toFixed(4).replace(/\.?0+$/, '') + " Gwei"
        return swapBackend.ethBalance + " wei"
    }

    // Pure-QML clipboard copy, mirroring ReceiptCard's mechanism (an invisible
    // TextEdit whose selection is copied). clipboardHelper/copiedReset live
    // inside the content Flickable below.
    function copyText(value, kind) {
        clipboardHelper.text = value
        clipboardHelper.selectAll()
        clipboardHelper.copy()
        clipboardHelper.text = ""
        setupRoot.copiedKind = kind
        copiedReset.restart()
    }

    // Elapsed-seconds ticker for the funding status line. Individual phases
    // (an Initialize commit, a claim commit) can each take a minute or more
    // of real chain time with no new event in between — without a moving
    // number the app looks frozen (0.4.1 feedback: "Checking account
    // initialization…" sat still for ~1 minute). Pure QML: ticks while the
    // job runs and resets whenever the backend reports a new step, so it
    // always reads "time spent in the CURRENT phase".
    property int setupStepElapsedSeconds: 0

    Flickable {
        contentHeight: col.implicitHeight + Theme.spacingXLarge * 2
        boundsBehavior: Flickable.StopAtBounds

        // The elapsed-seconds ticker and its reset hook live INSIDE the
        // Flickable on purpose. ScrollView adopts the FIRST Flickable in its
        // contentData as its scroll surface; a non-Flickable object appended
        // ahead of it (a Timer/Connections declared directly under setupRoot)
        // makes ScrollView spin up its own implicit Flickable and demote this
        // one to a zero-sized ordinary child — collapsing the whole layout to
        // the origin (the 0.4.2 Setup-tab overlap regression, #113). Keeping
        // these non-visual helpers here guarantees the Flickable stays the
        // ScrollView's sole direct content child.
        Timer {
            running: swapBackend.setupRunning
            interval: 1000
            repeat: true
            onTriggered: setupRoot.setupStepElapsedSeconds += 1
        }

        // Auto-detect Sepolia ETH arrival on the Get-test-ETH step: poll the
        // shared balance refresh while the user has an ETH key but no ETH yet,
        // so their funding shows up without a manual Refresh. triggeredOnStart
        // gives an immediate first read; the interval is deliberately slow and
        // the run is capped (ethPollMax) so it never hammers the RPC. It stops
        // itself the moment a positive balance lands (hasEthBalance).
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

        // Invisible clipboard surface + transient "copied" reset, mirroring
        // ReceiptCard. Non-visual helpers stay INSIDE this Flickable so they
        // never steal the root ScrollView's single content slot (#113).
        TextEdit {
            id: clipboardHelper
            visible: false
        }
        Timer {
            id: copiedReset
            interval: 1600
            onTriggered: setupRoot.copiedKind = ""
        }

        Connections {
            target: swapBackend
            function onSetupStepChanged() { setupRoot.setupStepElapsedSeconds = 0 }
            function onSetupRunningChanged() { setupRoot.setupStepElapsedSeconds = 0 }
            // A freshly generated key starts from zero ETH — restart the poll
            // budget so arrival on the new address is detected.
            function onEthRecipientAddressChanged() { setupRoot.ethPollCount = 0 }
        }

        ColumnLayout {
            id: col
            anchors {
                top: parent.top
                left: parent.left
                right: parent.right
                margins: Theme.spacingXLarge
            }
            spacing: Theme.spacingNormal

            Text {
                text: "Get set up"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            // NOTE (here and on every wrapped label below): alignment is
            // pinned to AlignLeft and wrapping to WordWrap explicitly. Left
            // implicit, the host app's ambient text defaults leaked in and
            // rendered these full-width labels justified — huge inter-word
            // gaps (0.4.1 live feedback).
            Text {
                text: "A few steps, no hand-typed keys: generate an ETH key, create a LEZ account, fund LEZ, then add a little Sepolia test-ETH for gas."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontNormal
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }

            // --- Step 1: ETH key ---
            Rectangle {
                Layout.fillWidth: true
                Layout.topMargin: Theme.spacingLarge
                implicitHeight: ethCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: setupRoot.hasEthKey ? Theme.success : Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: ethCol
                    anchors { fill: parent; margins: Theme.spacingNormal }
                    spacing: 6

                    SectionHeader {
                        label: "1. Ethereum key"
                        detail: setupRoot.hasEthKey ? "done" : ""
                        hairline: false
                    }
                    Text {
                        text: setupRoot.hasEthKey
                              ? ("Address: " + swapBackend.ethRecipientAddress)
                              : "No key yet. Generate one, or paste an existing key in the Config tab."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        font.family: setupRoot.hasEthKey ? Theme.monoFont : ""
                        horizontalAlignment: Text.AlignLeft
                        // Wrap (not WordWrap): the done-state shows one long
                        // unbreakable hex token that must break mid-token
                        // rather than clip on narrow cards.
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                    GhostButton {
                        text: setupRoot.hasEthKey ? "Generate a different key" : "Generate new key"
                        enabled: !setupRoot.anyRunning
                        Layout.preferredHeight: 38
                        font.bold: true
                        onClicked: swapBackend.setupGenerateEthKey()
                    }
                }
            }

            // --- Step 2: LEZ account ---
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: lezCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: setupRoot.hasLezAccount ? Theme.success : Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: lezCol
                    anchors { fill: parent; margins: Theme.spacingNormal }
                    spacing: 6

                    SectionHeader {
                        label: "2. LEZ account"
                        detail: setupRoot.hasLezAccount ? "done" : ""
                        hairline: false
                    }
                    Text {
                        text: setupRoot.hasLezAccount
                              ? ("Account: " + (swapBackend.lezAccount !== "" ? swapBackend.lezAccount : "(refreshing…)"))
                              : "No account yet. Create one — nothing is on-chain until step 3 funds it."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        font.family: setupRoot.hasLezAccount ? Theme.monoFont : ""
                        horizontalAlignment: Text.AlignLeft
                        // Wrap (not WordWrap): base58 account IDs are one
                        // long token — see the ETH-address label above.
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                    GhostButton {
                        text: setupRoot.hasLezAccount ? "Generate a different account" : "Create LEZ account"
                        enabled: !setupRoot.anyRunning
                        Layout.preferredHeight: 38
                        font.bold: true
                        onClicked: swapBackend.setupGenerateLezAccount()
                    }
                }
            }

            // --- Step 3: Fund ---
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: fundCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: setupRoot.lezFunded ? Theme.success : Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: fundCol
                    anchors { fill: parent; margins: Theme.spacingNormal }
                    spacing: 6

                    SectionHeader {
                        label: "3. Fund LEZ"
                        detail: setupRoot.lezFunded ? "done" : ""
                        hairline: false
                    }
                    Text {
                        text: "Initializes the account on-chain, then funds it in 150-LEZ faucet claims "
                              + "up to " + (swapBackend.setupTarget !== "" ? swapBackend.setupTarget : "150") + " LEZ. "
                              + "Each claim needs a proof-of-work solve plus an on-chain commit, and testnet "
                              + "blocks can be a minute or more apart — the timer below keeps counting while "
                              + "it works."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }

                    PrimaryButton {
                        text: swapBackend.setupRunning
                              ? "Setting up…"
                              : (setupRoot.lezFunded ? "Fund again" : "Fund my account")
                        enabled: setupRoot.hasLezAccount && !swapBackend.setupRunning && !setupRoot.anyRunning
                        Layout.preferredHeight: 42
                        onClicked: swapBackend.setupStartFunding()
                    }

                    RowLayout {
                        visible: swapBackend.setupRunning || swapBackend.setupStep !== ""
                        Layout.fillWidth: true
                        spacing: Theme.spacingSmall

                        BusyIndicator {
                            visible: swapBackend.setupRunning
                            running: swapBackend.setupRunning
                            implicitWidth: 18
                            implicitHeight: 18
                        }
                        Text {
                            Layout.fillWidth: true
                            horizontalAlignment: Text.AlignLeft
                            wrapMode: Text.WordWrap
                            text: {
                                var parts = [setupRoot.humanSetupStep(swapBackend.setupStep)]
                                // Live per-phase elapsed counter, so a slow
                                // chain phase visibly ticks instead of
                                // looking hung.
                                if (swapBackend.setupRunning && setupRoot.setupStepElapsedSeconds > 0)
                                    parts[0] += " " + setupRoot.setupStepElapsedSeconds + "s"
                                if (swapBackend.setupBalance !== "")
                                    parts.push(swapBackend.setupBalance + " / " + swapBackend.setupTarget + " LEZ")
                                if (swapBackend.setupClaims > 0)
                                    parts.push(swapBackend.setupClaims + " claim" + (swapBackend.setupClaims === 1 ? "" : "s") + " committed")
                                return parts.join(" — ")
                            }
                            color: Theme.textSecondary
                            font.pixelSize: Theme.fontSmall
                        }
                    }

                    Text {
                        visible: swapBackend.setupError !== ""
                        text: swapBackend.setupError
                        color: Theme.error
                        font.pixelSize: Theme.fontCaption
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                }
            }

            // --- Step 4: Get test ETH ---
            // The app's faucet funds LEZ only. The Ethereum side needs gas for
            // every swap, and there's no free programmatic Sepolia faucet — so
            // this step GUIDES the user to fund it themselves and auto-detects
            // arrival. Without it, a fresh account read "Funded and ready" with
            // 0 Sepolia ETH and the first swap died on "insufficient funds for
            // gas" (every 0.4.3 tester hit this).
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: ethFundCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: setupRoot.hasEthBalance ? Theme.success : Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: ethFundCol
                    anchors { fill: parent; margins: Theme.spacingNormal }
                    spacing: 6

                    SectionHeader {
                        label: "4. Get test ETH"
                        detail: setupRoot.hasEthBalance ? "done" : ""
                        hairline: false
                    }
                    Text {
                        text: "Send a little Sepolia test-ETH to this address to pay for swaps — "
                              + "the app funds LEZ but not Ethereum. A few hundredths of an ETH is plenty."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }

                    // Your ETH address + Copy — also closes the standing
                    // "Setup ETH address has no copy button" gap.
                    Text {
                        text: setupRoot.hasEthKey
                              ? swapBackend.ethRecipientAddress
                              : "Generate an Ethereum key in step 1 first."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        font.family: setupRoot.hasEthKey ? Theme.monoFont : ""
                        horizontalAlignment: Text.AlignLeft
                        // Wrap (not WordWrap): the address is one long
                        // unbreakable hex token — must break mid-token.
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                    GhostButton {
                        visible: setupRoot.hasEthKey
                        text: setupRoot.copiedKind === "ethAddress" ? "Address copied" : "Copy address"
                        enabled: !setupRoot.anyRunning
                        Layout.preferredHeight: 38
                        font.bold: true
                        onClicked: setupRoot.copyText(swapBackend.ethRecipientAddress, "ethAddress")
                    }

                    // Faucet link is COPY-only: Basecamp silently no-ops
                    // module-owned external navigation (#84), so a clickable
                    // "open" would do nothing. Copy it and paste in a browser —
                    // the same idiom the receipts use for their links.
                    Text {
                        text: "Sepolia faucet (copy and paste in your browser):"
                        color: Theme.textMuted
                        font.pixelSize: Theme.fontCaption
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        Layout.topMargin: Theme.spacingSmall
                    }
                    Text {
                        text: setupRoot.sepoliaFaucetUrl
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        font.family: Theme.monoFont
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }
                    GhostButton {
                        text: setupRoot.copiedKind === "faucet" ? "Faucet link copied" : "Copy faucet link"
                        enabled: !setupRoot.anyRunning
                        Layout.preferredHeight: 38
                        onClicked: setupRoot.copyText(setupRoot.sepoliaFaucetUrl, "faucet")
                    }

                    // Live arrival status — auto-polled while this step is
                    // active (ethBalancePoll Timer), so funding is detected
                    // without a manual Refresh. Honest until it lands: never
                    // claims ready on LEZ alone.
                    RowLayout {
                        visible: setupRoot.hasEthKey
                        Layout.fillWidth: true
                        Layout.topMargin: Theme.spacingSmall
                        spacing: Theme.spacingSmall

                        BusyIndicator {
                            visible: !setupRoot.hasEthBalance
                            running: !setupRoot.hasEthBalance
                            implicitWidth: 18
                            implicitHeight: 18
                        }
                        Text {
                            Layout.fillWidth: true
                            horizontalAlignment: Text.AlignLeft
                            wrapMode: Text.WordWrap
                            text: setupRoot.hasEthBalance
                                  ? "✓ ETH received — " + setupRoot.ethBalanceDisplay()
                                  : "Waiting for ETH… (checks automatically)"
                            color: setupRoot.hasEthBalance ? Theme.success : Theme.textSecondary
                            font.pixelSize: Theme.fontSmall
                        }
                    }
                }
            }

            // --- Step 5: Done ---
            Rectangle {
                Layout.fillWidth: true
                visible: setupRoot.isReady
                implicitHeight: doneCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.success
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: doneCol
                    anchors { fill: parent; margins: Theme.spacingNormal }
                    spacing: 6

                    SectionHeader {
                        label: "5. Done"
                        hairline: false
                    }
                    Text {
                        text: "You're set up — LEZ funded and Ethereum has gas. Head to the market to browse offers, or fine-tune anything in Config."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        horizontalAlignment: Text.AlignLeft
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                    PrimaryButton {
                        text: "Go to Market"
                        Layout.preferredHeight: 42
                        onClicked: setupRoot.finished()
                    }
                }
            }
        }
    }
}
