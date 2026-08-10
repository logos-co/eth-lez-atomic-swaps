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
    readonly property bool isFunded: swapBackend.setupStep === "Done"
    readonly property bool anyRunning: swapBackend.makerRunning || swapBackend.takerRunning || swapBackend.autoAcceptRunning

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
            "Done":               "Funded and ready",
        }
        return map[step] || step
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

        Connections {
            target: swapBackend
            function onSetupStepChanged() { setupRoot.setupStepElapsedSeconds = 0 }
            function onSetupRunningChanged() { setupRoot.setupStepElapsedSeconds = 0 }
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
                text: "Three steps, no hand-typed keys: generate an ETH key, create a LEZ account, then fund it."
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
                border.color: setupRoot.isFunded ? Theme.success : Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: fundCol
                    anchors { fill: parent; margins: Theme.spacingNormal }
                    spacing: 6

                    SectionHeader {
                        label: "3. Fund it"
                        detail: setupRoot.isFunded ? "done" : ""
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
                              : (setupRoot.isFunded ? "Fund again" : "Fund my account")
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

            // --- Step 4: Done ---
            Rectangle {
                Layout.fillWidth: true
                visible: setupRoot.isFunded
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
                        label: "4. Done"
                        hairline: false
                    }
                    Text {
                        text: "You're set up. Head to the market to browse offers, or fine-tune anything in Config."
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
