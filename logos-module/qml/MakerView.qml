import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: makerRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    property var makerSteps: [
        { name: "EthLockDetected",   label: "Wait for ETH Lock" },
        { name: "LezLocked",        label: "Lock LEZ" },
        { name: "PreimageRevealed",  label: "Wait for Preimage" },
        { name: "EthClaimed",        label: "Claim ETH" },
    ]

    // Track completed steps based on progress events
    property var completedSteps: {
        var done = []
        var steps = swapBackend.makerProgressSteps
        for (var i = 0; i < steps.length; i++) {
            if (done.indexOf(steps[i]) < 0)
                done.push(steps[i])
        }
        return done
    }

    property string cumulativeStats: {
        var n = swapBackend.autoAcceptCompleted
        if (n <= 0) return ""
        var lezSold = n * Number(swapBackend.lezAmount)
        var ethEarned = n * Number(swapBackend.ethAmount)
        return n + " swap" + (n > 1 ? "s" : "") + " completed (" + lezSold + " LEZ sold for " + ethEarned + " ETH)"
    }

    property string statusText: {
        if (!swapBackend.autoAcceptRunning) {
            if (makerRoot.cumulativeStats)
                return "Offline \u2014 " + makerRoot.cumulativeStats
            return "Set your rate and go live to start accepting swaps"
        }
        if (swapBackend.makerCurrentStep === "" || swapBackend.makerCurrentStep === "WaitingForEthLock") {
            if (swapBackend.autoAcceptCompleted === 0)
                return "\u25CF LIVE \u2014 Listening for buyers..."
            return "\u25CF LIVE \u2014 " + makerRoot.cumulativeStats + " \u2014 listening for buyers..."
        }
        return "\u25CF LIVE \u2014 Processing swap..."
    }

    function timeAgo(timestampMs) {
        if (!timestampMs) return ""
        var diff = Date.now() - timestampMs
        if (diff < 0) diff = 0
        var sec = Math.floor(diff / 1000)
        if (sec < 60) return sec + "s ago"
        var min = Math.floor(sec / 60)
        if (min < 60) return min + "m ago"
        var hr = Math.floor(min / 60)
        return hr + "h " + (min % 60) + "m ago"
    }

    Flickable {
        contentHeight: makerCol.implicitHeight + Theme.spacingXLarge * 2
        boundsBehavior: Flickable.StopAtBounds

        ColumnLayout {
            id: makerCol
            anchors {
                top: parent.top
                left: parent.left
                right: parent.right
                margins: Theme.spacingXLarge
            }
            spacing: Theme.spacingLarge

            Text {
                text: "Sell LEZ"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }

            // Auto-refresh balances after each swap
            Connections {
                target: swapBackend
                function onSwapHistoryChanged() {
                    swapBackend.fetchBalances()
                }
            }

            // --- Your Offer summary card ---
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: offerCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: offerCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: 6

                    Text {
                        text: "Your Rate"
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontNormal
                        font.bold: true
                    }
                    Text {
                        text: swapBackend.lezAmount + " LEZ \u2192 " + swapBackend.ethAmount + " ETH  per swap"
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontNormal
                    }
                    Text {
                        text: {
                            var bal = swapBackend.lezBalance
                            var amt = Number(swapBackend.lezAmount)
                            var n = (amt > 0) ? Math.floor(Number(bal) / amt) : 0
                            return "Available: " + bal + " LEZ" + (n > 0 ? "  (~" + n + " swaps at this rate)" : "")
                        }
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                    }
                }
            }

            // --- Go Live Toggle ---
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: goLiveCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: swapBackend.autoAcceptRunning ? Theme.accent : Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: goLiveCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: 6

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Theme.spacingNormal

                        Text {
                            text: "Go Live"
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontNormal
                            font.bold: true
                        }

                        Item { Layout.fillWidth: true }

                        Switch {
                            checked: swapBackend.autoAcceptRunning
                            enabled: !swapBackend.makerRunning
                            onToggled: {
                                if (checked) {
                                    swapBackend.startAutoAccept()
                                } else {
                                    swapBackend.stopAutoAccept()
                                }
                            }
                        }
                    }

                    Text {
                        text: "Continuously accept swaps at this rate until stopped"
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                    }
                }
            }

            // --- Contextual Status Text ---
            Text {
                text: makerRoot.statusText
                color: swapBackend.autoAcceptRunning ? Theme.accent : Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // --- Progress (only visible during active swap) ---
            Rectangle {
                visible: swapBackend.autoAcceptRunning && swapBackend.makerCurrentStep !== "" && swapBackend.makerCurrentStep !== "WaitingForEthLock"
                Layout.fillWidth: true
                implicitHeight: makerStepper.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ProgressStepper {
                    id: makerStepper
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    steps: makerSteps
                    currentStep: swapBackend.makerCurrentStep
                    completedSteps: makerRoot.completedSteps
                }
            }

            // Result
            ResultCard {
                resultJson: swapBackend.makerResultJson
            }

            // --- Completed Swaps ---
            Rectangle {
                visible: swapBackend.swapHistory.length > 0
                Layout.fillWidth: true
                implicitHeight: historyCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: historyCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: 8

                    Text {
                        text: "Completed Swaps (" + swapBackend.swapHistory.length + ")"
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontNormal
                        font.bold: true
                    }

                    Repeater {
                        model: swapBackend.swapHistory
                        delegate: Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: entryCol.implicitHeight + 12
                            color: "transparent"
                            border.color: Theme.border
                            border.width: 1
                            radius: Theme.radiusSmall

                            ColumnLayout {
                                id: entryCol
                                anchors {
                                    fill: parent
                                    margins: 6
                                }
                                spacing: 4

                                // Parse the JSON entry
                                property var entry: {
                                    try { return JSON.parse(modelData) }
                                    catch(e) { return { status: "unknown" } }
                                }

                                RowLayout {
                                    Layout.fillWidth: true
                                    Text {
                                        text: {
                                            if (entryCol.entry.status === "completed")
                                                return "Sold " + entryCol.entry.lez_amount + " LEZ for " + entryCol.entry.eth_amount + " ETH"
                                            if (entryCol.entry.status === "failed")
                                                return "Failed"
                                            if (entryCol.entry.status === "insufficient_funds")
                                                return "Insufficient funds"
                                            return entryCol.entry.status
                                        }
                                        color: entryCol.entry.status === "completed" ? Theme.accent : Theme.error
                                        font.pixelSize: Theme.fontSmall
                                        font.bold: true
                                    }
                                    Item { Layout.fillWidth: true }
                                    Text {
                                        text: makerRoot.timeAgo(entryCol.entry.timestamp)
                                        color: Theme.textMuted
                                        font.pixelSize: 11
                                    }
                                }

                                // Completed: show tx hashes
                                Text {
                                    visible: entryCol.entry.status === "completed" && (entryCol.entry.eth_tx || entryCol.entry.lez_tx)
                                    text: entryCol.entry.eth_tx ? "ETH: " + entryCol.entry.eth_tx.substring(0, 10) + "..." + entryCol.entry.eth_tx.substring(entryCol.entry.eth_tx.length - 5) : ""
                                    color: Theme.textMuted
                                    font.pixelSize: 11
                                    font.family: "Menlo, Courier New"
                                }
                                Text {
                                    visible: entryCol.entry.status === "completed" && entryCol.entry.lez_tx
                                    text: entryCol.entry.lez_tx ? "LEZ: " + entryCol.entry.lez_tx.substring(0, 10) + "..." + entryCol.entry.lez_tx.substring(entryCol.entry.lez_tx.length - 5) : ""
                                    color: Theme.textMuted
                                    font.pixelSize: 11
                                    font.family: "Menlo, Courier New"
                                }

                                // Failed: show error
                                Text {
                                    visible: entryCol.entry.status === "failed" && entryCol.entry.error
                                    text: entryCol.entry.error || ""
                                    color: Theme.textMuted
                                    font.pixelSize: 11
                                    wrapMode: Text.Wrap
                                    Layout.fillWidth: true
                                }

                                // Insufficient funds: show balance info
                                Text {
                                    visible: entryCol.entry.status === "insufficient_funds"
                                    text: "Have " + (entryCol.entry.lez_balance || "?") + " LEZ, need " + (entryCol.entry.lez_required || "?") + " LEZ"
                                    color: Theme.textMuted
                                    font.pixelSize: 11
                                    wrapMode: Text.Wrap
                                    Layout.fillWidth: true
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
