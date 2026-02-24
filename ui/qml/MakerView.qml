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

    // Extract hashlock from progress events
    property string hashlock: {
        // Not directly available from progressSteps (just step names).
        // We'd need the full JSON. For now, display from the step name hint.
        return ""
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
                text: "Maker Flow"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            Text {
                text: "Generate preimage, lock LEZ, wait for taker to lock ETH, then claim ETH."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // Start button
            Button {
                text: swapBackend.running ? "Running..." : "Start Maker"
                enabled: !swapBackend.running
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

                onClicked: swapBackend.startMaker()
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
