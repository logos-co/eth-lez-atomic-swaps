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

    property bool messagingEnabled: swapBackend.nwakuUrl !== ""
    property bool offerPublished: false
    property bool publishing: false

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

            Connections {
                target: swapBackend
                function onOfferPublished(resultJson) {
                    makerRoot.publishing = false
                    var obj = JSON.parse(resultJson)
                    if (obj.ok) {
                        makerRoot.offerPublished = true
                    }
                }
            }

            Text {
                text: "Maker Flow"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            Text {
                text: messagingEnabled
                    ? "Publish offer, then wait for taker to lock ETH. Lock LEZ and claim ETH."
                    : "Wait for taker to lock ETH, lock LEZ, wait for preimage, claim ETH."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // --- Step 1: Publish Offer (messaging enabled) ---
            Button {
                visible: messagingEnabled && !offerPublished
                text: publishing ? "Publishing..." : "Publish Offer"
                enabled: !publishing && !swapBackend.makerRunning
                Layout.fillWidth: true
                Layout.preferredHeight: 48
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
                    color: parent.enabled ? "#ffffff" : Theme.textMuted
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    font: parent.font
                }

                onClicked: {
                    makerRoot.publishing = true
                    swapBackend.publishOffer()
                }
            }

            // Published offer card
            Rectangle {
                visible: messagingEnabled && offerPublished
                Layout.fillWidth: true
                implicitHeight: offerCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.accent
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
                        text: "Offer Published"
                        color: Theme.accent
                        font.pixelSize: Theme.fontNormal
                        font.bold: true
                    }
                    Text {
                        text: "Waiting for taker to accept and lock ETH..."
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        wrapMode: Text.WrapAnywhere
                        Layout.fillWidth: true
                    }
                }
            }

            // --- Auto-Accept Toggle ---
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: autoAcceptRow.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                RowLayout {
                    id: autoAcceptRow
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: Theme.spacingNormal

                    Text {
                        text: "Auto-Accept"
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
            }

            // --- Auto-Accept Stats ---
            Rectangle {
                visible: swapBackend.autoAcceptRunning || swapBackend.autoAcceptIteration > 0
                Layout.fillWidth: true
                implicitHeight: statsCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: statsCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: 6

                    RowLayout {
                        spacing: Theme.spacingLarge
                        Text {
                            text: "Iteration: " + swapBackend.autoAcceptIteration
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontSmall
                        }
                        Text {
                            text: "Completed: " + swapBackend.autoAcceptCompleted
                            color: Theme.accent
                            font.pixelSize: Theme.fontSmall
                        }
                        Text {
                            text: "Failed: " + swapBackend.autoAcceptFailed
                            color: swapBackend.autoAcceptFailed > 0 ? Theme.error : Theme.textSecondary
                            font.pixelSize: Theme.fontSmall
                        }
                    }
                }
            }

            // --- Step 2: Start Single Swap (manual) ---
            Button {
                text: swapBackend.makerRunning ? "Running..." : (messagingEnabled && offerPublished ? "Start Swap" : "Start Single Swap")
                enabled: !swapBackend.makerRunning && !swapBackend.autoAcceptRunning && (!messagingEnabled || offerPublished)
                visible: !swapBackend.autoAcceptRunning && (!messagingEnabled || offerPublished)
                Layout.fillWidth: true
                Layout.preferredHeight: 48
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
                    color: parent.enabled ? "#ffffff" : Theme.textMuted
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    font: parent.font
                }

                onClicked: {
                    swapBackend.startMaker("")
                }
            }

            // Progress
            Rectangle {
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

            // --- Swap History ---
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
                    spacing: 6

                    Text {
                        text: "Swap History"
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontNormal
                        font.bold: true
                    }

                    Repeater {
                        model: swapBackend.swapHistory
                        delegate: Text {
                            text: modelData
                            color: modelData.indexOf("completed") >= 0 ? Theme.accent : Theme.textSecondary
                            font.pixelSize: Theme.fontSmall
                            wrapMode: Text.Wrap
                            Layout.fillWidth: true
                        }
                    }
                }
            }
        }
    }
}
