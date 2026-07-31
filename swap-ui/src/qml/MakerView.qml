import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme

ScrollView {
    id: makerRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    property var makerSteps: [
        { name: "WaitingForEthLock", label: "Wait for ETH Lock" },
        { name: "EthLockDetected",   label: "ETH Lock Detected" },
        { name: "LezLocking",        label: "Lock LEZ" },
        { name: "LezLocked",         label: "LEZ Locked" },
        { name: "WaitingForPreimage", label: "Wait for Preimage" },
        { name: "PreimageRevealed",  label: "Preimage Revealed" },
        { name: "ClaimingEth",       label: "Claim ETH" },
        { name: "EthClaimed",        label: "ETH Claimed" },
    ]

    // Track completed steps based on progress events
    property var completedSteps: {
        var done = []
        var steps = swapBackend.makerProgressSteps
        for (var i = 0; i < steps.length; i++) {
            if (steps[i] !== swapBackend.makerCurrentStep && done.indexOf(steps[i]) < 0)
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
            return "Set your rate and go live to publish an actionable offer"
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

            RowLayout {
                Layout.fillWidth: true
                spacing: Theme.spacingNormal

                Text {
                    text: "Sell LEZ"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontTitle
                    font.bold: true
                }
                StatusChip {
                    text: swapBackend.autoAcceptRunning ? "Live" : "Offline"
                    tone: swapBackend.autoAcceptRunning ? Theme.success : Theme.textMuted
                    pulsing: swapBackend.autoAcceptRunning
                    bold: swapBackend.autoAcceptRunning
                }
                Item { Layout.fillWidth: true }
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

                    SectionHeader {
                        label: "Your rate"
                        hairline: false
                    }
                    RowLayout {
                        spacing: 6

                        Text {
                            text: swapBackend.lezAmount + " LEZ"
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontLarge
                            font.bold: true
                            font.family: Theme.monoFont
                        }
                        Text {
                            text: "\u21c4"
                            color: Theme.textMuted
                            font.pixelSize: Theme.fontNormal
                        }
                        Text {
                            text: swapBackend.ethAmount + " ETH"
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontLarge
                            font.bold: true
                            font.family: Theme.monoFont
                        }
                        Text {
                            text: "per swap"
                            color: Theme.textMuted
                            font.pixelSize: Theme.fontSmall
                        }
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

            // --- Go Live Action ---
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

                    SectionHeader {
                        label: "Live maker"
                        hairline: false
                    }

                    Text {
                        text: "Publishes your current rate as an actionable offer and starts listening for buyer ETH locks."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
                    }

                    Button {
                        id: goLiveButton
                        text: swapBackend.messagingRetrying && !swapBackend.autoAcceptRunning
                              ? "Waiting for Delivery..."
                              : (swapBackend.autoAcceptRunning ? "Stop Live Maker" : "Go Live & Publish Offer")
                        enabled: swapBackend.autoAcceptRunning
                                 ? !swapBackend.messagingLoading
                                 : (!swapBackend.makerRunning && !swapBackend.takerRunning && !swapBackend.messagingLoading
                                    && !swapBackend.messagingRetrying
                                    )
                        Layout.fillWidth: true
                        Layout.preferredHeight: 42
                        font.pixelSize: Theme.fontNormal
                        font.bold: true

                        // Filled accent to go live; error-outlined ghost to stop.
                        background: Rectangle {
                            color: swapBackend.autoAcceptRunning
                                   ? (goLiveButton.enabled && goLiveButton.hovered
                                      ? Theme.surfaceLight : Theme.surface)
                                   : (goLiveButton.enabled
                                      ? (goLiveButton.hovered ? Theme.accentHover : Theme.accent)
                                      : Theme.surfaceLight)
                            border.color: swapBackend.autoAcceptRunning ? Theme.error : "transparent"
                            border.width: swapBackend.autoAcceptRunning ? 1 : 0
                            radius: Theme.radiusNormal
                        }
                        contentItem: Text {
                            text: goLiveButton.text
                            color: swapBackend.autoAcceptRunning
                                   ? Theme.error
                                   : (goLiveButton.enabled ? Theme.accentForeground : Theme.textMuted)
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                            font: goLiveButton.font
                        }

                        onClicked: {
                            if (swapBackend.autoAcceptRunning)
                                swapBackend.stopAutoAccept()
                            else
                                swapBackend.startAutoAccept()
                        }
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
                visible: swapBackend.autoAcceptRunning && swapBackend.makerCurrentStep !== ""
                Layout.fillWidth: true
                implicitHeight: makerProgressCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: makerProgressCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: Theme.spacingSmall

                    SectionHeader {
                        label: "Swap in progress"
                        hairline: false
                    }
                    ProgressStepper {
                        id: makerStepper
                        Layout.fillWidth: true
                        steps: makerSteps
                        currentStep: swapBackend.makerCurrentStep
                        completedSteps: makerRoot.completedSteps
                    }
                }
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

                    SectionHeader {
                        label: "Completed swaps"
                        detail: "" + swapBackend.swapHistory.length
                        hairline: false
                    }

                    Repeater {
                        model: swapBackend.swapHistory
                        delegate: Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: entryCol.implicitHeight + 12
                            color: "transparent"

                            // Hairline separator (tape idiom)
                            Rectangle {
                                anchors.top: parent.top
                                width: parent.width
                                height: 1
                                color: Theme.border
                                opacity: 0.6
                            }

                            ColumnLayout {
                                id: entryCol
                                anchors {
                                    fill: parent
                                    topMargin: 8
                                    bottomMargin: 4
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
                                        color: entryCol.entry.status === "completed" ? Theme.success : Theme.error
                                        font.pixelSize: Theme.fontSmall
                                        font.bold: true
                                    }
                                    Item { Layout.fillWidth: true }
                                    Text {
                                        text: makerRoot.timeAgo(entryCol.entry.timestamp)
                                        color: Theme.textMuted
                                        font.pixelSize: Theme.fontCaption
                                        font.family: Theme.monoFont
                                    }
                                }

                                // Completed: show tx hashes
                                Text {
                                    visible: entryCol.entry.status === "completed" && (entryCol.entry.eth_tx || entryCol.entry.lez_tx)
                                    text: entryCol.entry.eth_tx ? "ETH: " + entryCol.entry.eth_tx.substring(0, 10) + "..." + entryCol.entry.eth_tx.substring(entryCol.entry.eth_tx.length - 5) : ""
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.fontCaption
                                    font.family: Theme.monoFont
                                }
                                Text {
                                    visible: entryCol.entry.status === "completed" && entryCol.entry.lez_tx
                                    text: entryCol.entry.lez_tx ? "LEZ: " + entryCol.entry.lez_tx.substring(0, 10) + "..." + entryCol.entry.lez_tx.substring(entryCol.entry.lez_tx.length - 5) : ""
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.fontCaption
                                    font.family: Theme.monoFont
                                }

                                // Failed: show error
                                Text {
                                    visible: entryCol.entry.status === "failed" && entryCol.entry.error
                                    text: entryCol.entry.error || ""
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.fontCaption
                                    wrapMode: Text.Wrap
                                    Layout.fillWidth: true
                                }

                                // Insufficient funds: show balance info
                                Text {
                                    visible: entryCol.entry.status === "insufficient_funds"
                                    text: "Have " + (entryCol.entry.lez_balance || "?") + " LEZ, need " + (entryCol.entry.lez_required || "?") + " LEZ"
                                    color: Theme.textMuted
                                    font.pixelSize: Theme.fontCaption
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
