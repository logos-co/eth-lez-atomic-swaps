import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: takerRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    property var takerSteps: [
        { name: "LezEscrowVerified", label: "Verify LEZ Escrow" },
        { name: "EthLocked",         label: "Lock ETH" },
        { name: "PreimageRevealed",  label: "Wait for Preimage" },
        { name: "LezClaimed",        label: "Claim LEZ" },
    ]

    property var completedSteps: {
        var done = []
        var steps = swapBackend.progressSteps
        for (var i = 0; i < steps.length; i++) {
            if (done.indexOf(steps[i]) < 0)
                done.push(steps[i])
        }
        return done
    }

    Flickable {
        contentHeight: takerCol.implicitHeight + Theme.spacingXLarge * 2
        boundsBehavior: Flickable.StopAtBounds

        ColumnLayout {
            id: takerCol
            anchors {
                top: parent.top
                left: parent.left
                right: parent.right
                margins: Theme.spacingXLarge
            }
            spacing: Theme.spacingLarge

            Text {
                text: "Taker Flow"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            Text {
                text: "Verify maker's LEZ escrow, lock ETH, wait for maker to reveal preimage, then claim LEZ."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // Hashlock input
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4

                Text {
                    text: "Hashlock (from maker)"
                    color: Theme.textSecondary
                    font.pixelSize: Theme.fontSmall
                }
                TextField {
                    id: hashlockInput
                    Layout.fillWidth: true
                    Layout.preferredHeight: Theme.inputHeight
                    leftPadding: 12
                    rightPadding: 12
                    topPadding: 8
                    bottomPadding: 8
                    placeholderText: "64-char hex (e.g. abcd1234...)"
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontNormal
                    font.family: "monospace"
                    selectByMouse: true
                    maximumLength: 66 // 0x prefix + 64 hex
                    background: Rectangle {
                        color: Theme.inputBackground
                        border.color: {
                            if (!hashlockInput.activeFocus) return Theme.border
                            return hashlockInput.text.length > 0 && !isValidHex(hashlockInput.text, 32)
                                   ? Theme.error : Theme.accent
                        }
                        border.width: 1
                        radius: Theme.radiusSmall
                    }
                }
                Text {
                    visible: hashlockInput.text.length > 0 && !isValidHex(hashlockInput.text, 32)
                    text: "Must be 64 hex characters (32 bytes)"
                    color: Theme.error
                    font.pixelSize: 11
                }
            }

            function isValidHex(s, bytes) {
                let clean = s.startsWith("0x") ? s.substring(2) : s
                if (clean.length !== bytes * 2) return false
                return /^[0-9a-fA-F]+$/.test(clean)
            }

            // Start button
            Button {
                text: swapBackend.running ? "Running..." : "Start Taker"
                enabled: !swapBackend.running && isValidHex(hashlockInput.text, 32)
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

                onClicked: swapBackend.startTaker(hashlockInput.text)
            }

            // Progress
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: takerStepper.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.border
                border.width: 1
                radius: Theme.radiusNormal

                ProgressStepper {
                    id: takerStepper
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    steps: takerSteps
                    currentStep: swapBackend.currentStep
                    completedSteps: takerRoot.completedSteps
                }
            }

            // Result
            ResultCard {
                resultJson: swapBackend.resultJson
            }
        }
    }
}
