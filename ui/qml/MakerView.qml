import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: makerRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    property var makerSteps: [
        { name: "PreimageGenerated", label: "Generate Preimage" },
        { name: "LezLocked",        label: "Lock LEZ" },
        { name: "EthLockDetected",   label: "Wait for ETH Lock" },
        { name: "EthClaimed",        label: "Claim ETH" },
    ]

    // Track completed steps based on progress events
    property var completedSteps: {
        var done = []
        var steps = swapBackend.progressSteps
        for (var i = 0; i < steps.length; i++) {
            if (done.indexOf(steps[i]) < 0)
                done.push(steps[i])
        }
        return done
    }

    property bool messagingEnabled: swapBackend.nwakuUrl !== ""
    property string publishedHashlock: ""
    property string publishedPreimage: ""
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
                        makerRoot.publishedHashlock = obj.hashlock
                        makerRoot.publishedPreimage = obj.preimage
                        makerRoot.offerPublished = true
                    } else {
                        makerRoot.publishedHashlock = "Error: " + (obj.error || "unknown")
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
                    ? "Publish offer via messaging, then lock LEZ and wait for taker."
                    : "Generate preimage, lock LEZ, wait for taker to lock ETH, then claim ETH."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // --- Step 1: Publish Offer (messaging enabled) ---
            Button {
                visible: messagingEnabled && !offerPublished
                text: publishing ? "Publishing..." : "Publish Offer"
                enabled: !publishing && !swapBackend.running
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
                        text: "Hashlock: " + publishedHashlock
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                        font.family: "Menlo, Courier New"
                        wrapMode: Text.WrapAnywhere
                        Layout.fillWidth: true
                    }
                }
            }

            // --- Step 2: Start Swap ---
            Button {
                text: swapBackend.running ? "Running..." : (messagingEnabled && offerPublished ? "Start Swap" : "Start Maker")
                enabled: !swapBackend.running && (!messagingEnabled || offerPublished)
                visible: !messagingEnabled || offerPublished
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
                    if (messagingEnabled && publishedPreimage !== "")
                        swapBackend.startMaker(publishedPreimage)
                    else
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
                    currentStep: swapBackend.currentStep
                    completedSteps: makerRoot.completedSteps
                }
            }

            // Result
            ResultCard {
                resultJson: swapBackend.resultJson
            }
        }
    }
}
