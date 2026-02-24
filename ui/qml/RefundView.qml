import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    Flickable {
        contentHeight: refundCol.implicitHeight + Theme.spacingXLarge * 2
        boundsBehavior: Flickable.StopAtBounds

        ColumnLayout {
            id: refundCol
            anchors {
                top: parent.top
                left: parent.left
                right: parent.right
                margins: Theme.spacingXLarge
            }
            spacing: Theme.spacingLarge

            Text {
                text: "Manual Refund"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            Text {
                text: "Refund expired HTLCs. LEZ refund checks timelock off-chain; ETH refund relies on on-chain enforcement."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            function isValidHex(s, bytes) {
                let clean = s.startsWith("0x") ? s.substring(2) : s
                if (clean.length !== bytes * 2) return false
                return /^[0-9a-fA-F]+$/.test(clean)
            }

            // --- LEZ Refund ---
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: lezCol.implicitHeight + Theme.spacingNormal * 2
                radius: Theme.radiusNormal
                color: Theme.surface
                border.color: Theme.border
                border.width: 1

                ColumnLayout {
                    id: lezCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: Theme.spacingSmall

                    Text {
                        text: "LEZ Refund"
                        color: Theme.accent
                        font.pixelSize: Theme.fontLarge
                        font.bold: true
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        Text {
                            text: "Hashlock"
                            color: Theme.textSecondary
                            font.pixelSize: Theme.fontSmall
                        }
                        TextField {
                            id: lezHashlockInput
                            Layout.fillWidth: true
                            Layout.preferredHeight: Theme.inputHeight
                            leftPadding: 12
                            rightPadding: 12
                            topPadding: 8
                            bottomPadding: 8
                            placeholderText: "64-char hex"
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontNormal
                            font.family: "monospace"
                            selectByMouse: true
                            background: Rectangle {
                                color: Theme.inputBackground
                                border.color: lezHashlockInput.activeFocus ? Theme.accent : Theme.border
                                border.width: 1
                                radius: Theme.radiusSmall
                            }
                        }
                    }

                    Button {
                        text: swapBackend.running ? "Running..." : "Refund LEZ"
                        enabled: !swapBackend.running && isValidHex(lezHashlockInput.text, 32)
                        Layout.fillWidth: true
                        Layout.preferredHeight: 40
                        font.pixelSize: Theme.fontNormal

                        background: Rectangle {
                            color: parent.enabled
                                   ? (parent.hovered ? Theme.accentHover : Theme.accent)
                                   : Theme.surfaceLight
                            radius: Theme.radiusSmall
                        }
                        contentItem: Text {
                            text: parent.text
                            color: parent.enabled ? "#ffffff" : Theme.textMuted
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                            font: parent.font
                        }

                        onClicked: swapBackend.refundLez(lezHashlockInput.text)
                    }
                }
            }

            // --- ETH Refund ---
            Rectangle {
                Layout.fillWidth: true
                implicitHeight: ethCol.implicitHeight + Theme.spacingNormal * 2
                radius: Theme.radiusNormal
                color: Theme.surface
                border.color: Theme.border
                border.width: 1

                ColumnLayout {
                    id: ethCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: Theme.spacingSmall

                    Text {
                        text: "ETH Refund"
                        color: Theme.accent
                        font.pixelSize: Theme.fontLarge
                        font.bold: true
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        Text {
                            text: "Swap ID"
                            color: Theme.textSecondary
                            font.pixelSize: Theme.fontSmall
                        }
                        TextField {
                            id: ethSwapIdInput
                            Layout.fillWidth: true
                            Layout.preferredHeight: Theme.inputHeight
                            leftPadding: 12
                            rightPadding: 12
                            topPadding: 8
                            bottomPadding: 8
                            placeholderText: "64-char hex"
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontNormal
                            font.family: "monospace"
                            selectByMouse: true
                            background: Rectangle {
                                color: Theme.inputBackground
                                border.color: ethSwapIdInput.activeFocus ? Theme.accent : Theme.border
                                border.width: 1
                                radius: Theme.radiusSmall
                            }
                        }
                    }

                    Button {
                        text: swapBackend.running ? "Running..." : "Refund ETH"
                        enabled: !swapBackend.running && isValidHex(ethSwapIdInput.text, 32)
                        Layout.fillWidth: true
                        Layout.preferredHeight: 40
                        font.pixelSize: Theme.fontNormal

                        background: Rectangle {
                            color: parent.enabled
                                   ? (parent.hovered ? Theme.accentHover : Theme.accent)
                                   : Theme.surfaceLight
                            radius: Theme.radiusSmall
                        }
                        contentItem: Text {
                            text: parent.text
                            color: parent.enabled ? "#ffffff" : Theme.textMuted
                            horizontalAlignment: Text.AlignHCenter
                            verticalAlignment: Text.AlignVCenter
                            font: parent.font
                        }

                        onClicked: swapBackend.refundEth(ethSwapIdInput.text)
                    }
                }
            }

            // Result
            ResultCard {
                resultJson: swapBackend.resultJson
            }
        }
    }
}
