import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: refundRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    function isValidHex(s, bytes) {
        let clean = s.startsWith("0x") ? s.substring(2) : s
        if (clean.length !== bytes * 2) return false
        return /^[0-9a-fA-F]+$/.test(clean)
    }

    // Parse last swap result to auto-populate refund fields
    readonly property var lastResult: {
        if (!swapBackend.resultJson) return null
        try { return JSON.parse(swapBackend.resultJson) }
        catch (e) { return null }
    }

    // hashlock is available in completed/refunded results
    readonly property string lastHashlock: lastResult ? (lastResult.hashlock || "") : ""
    // eth_tx from taker is the swap_id needed for ETH refund
    readonly property string lastEthSwapId: lastResult ? (lastResult.eth_tx || "") : ""

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
                text: "Reclaim funds from expired HTLCs. The maker refunds LEZ (needs hashlock), the taker refunds ETH (needs swap ID)."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // Auto-populated hint
            Rectangle {
                visible: refundRoot.lastHashlock !== "" || refundRoot.lastEthSwapId !== ""
                Layout.fillWidth: true
                implicitHeight: hintCol.implicitHeight + Theme.spacingSmall * 2
                color: "#1a2a1a"
                border.color: Theme.success
                border.width: 1
                radius: Theme.radiusSmall

                ColumnLayout {
                    id: hintCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingSmall
                    }
                    spacing: 4

                    Text {
                        text: "Auto-populated from last swap result"
                        color: Theme.success
                        font.pixelSize: 12
                        font.bold: true
                    }
                    Text {
                        visible: refundRoot.lastHashlock !== ""
                        text: "Hashlock: " + refundRoot.lastHashlock.substring(0, 16) + "..."
                        color: Theme.textMuted
                        font.pixelSize: 11
                        font.family: "Menlo, Courier New"
                    }
                    Text {
                        visible: refundRoot.lastEthSwapId !== ""
                        text: "Swap ID: " + refundRoot.lastEthSwapId.substring(0, 16) + "..."
                        color: Theme.textMuted
                        font.pixelSize: 11
                        font.family: "Menlo, Courier New"
                    }
                }
            }

            // --- LEZ Refund (Maker) ---
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
                        text: "LEZ Refund (Maker)"
                        color: Theme.accent
                        font.pixelSize: Theme.fontLarge
                        font.bold: true
                    }
                    Text {
                        text: "Reclaim LEZ locked in escrow after the LEZ timelock expires (default 10m). Timelock is checked off-chain."
                        color: Theme.textMuted
                        font.pixelSize: 12
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
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
                            placeholderText: "64-char hex (e.g. abcd1234...)"
                            text: refundRoot.lastHashlock
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontNormal
                            font.family: "Menlo, Courier New"
                            selectByMouse: true
                            maximumLength: 66
                            background: Rectangle {
                                color: Theme.inputBackground
                                border.color: {
                                    if (!lezHashlockInput.activeFocus) return Theme.border
                                    return lezHashlockInput.text.length > 0 && !refundRoot.isValidHex(lezHashlockInput.text, 32)
                                           ? Theme.error : Theme.accent
                                }
                                border.width: 1
                                radius: Theme.radiusSmall
                            }
                        }
                        Text {
                            visible: lezHashlockInput.text.length > 0 && !refundRoot.isValidHex(lezHashlockInput.text, 32)
                            text: "Must be 64 hex characters (32 bytes)"
                            color: Theme.error
                            font.pixelSize: 11
                        }
                    }

                    Button {
                        text: swapBackend.running ? "Running..." : "Refund LEZ"
                        enabled: !swapBackend.running && refundRoot.isValidHex(lezHashlockInput.text, 32)
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

            // --- ETH Refund (Taker) ---
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
                        text: "ETH Refund (Taker)"
                        color: Theme.accent
                        font.pixelSize: Theme.fontLarge
                        font.bold: true
                    }
                    Text {
                        text: "Reclaim ETH locked in the HTLC contract after the ETH timelock expires (default 5m). Enforced on-chain."
                        color: Theme.textMuted
                        font.pixelSize: 12
                        wrapMode: Text.Wrap
                        Layout.fillWidth: true
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
                            placeholderText: "64-char hex from ETH lock tx"
                            text: refundRoot.lastEthSwapId
                            color: Theme.textPrimary
                            font.pixelSize: Theme.fontNormal
                            font.family: "Menlo, Courier New"
                            selectByMouse: true
                            maximumLength: 66
                            background: Rectangle {
                                color: Theme.inputBackground
                                border.color: {
                                    if (!ethSwapIdInput.activeFocus) return Theme.border
                                    return ethSwapIdInput.text.length > 0 && !refundRoot.isValidHex(ethSwapIdInput.text, 32)
                                           ? Theme.error : Theme.accent
                                }
                                border.width: 1
                                radius: Theme.radiusSmall
                            }
                        }
                        Text {
                            visible: ethSwapIdInput.text.length > 0 && !refundRoot.isValidHex(ethSwapIdInput.text, 32)
                            text: "Must be 64 hex characters (32 bytes)"
                            color: Theme.error
                            font.pixelSize: 11
                        }
                    }

                    Button {
                        text: swapBackend.running ? "Running..." : "Refund ETH"
                        enabled: !swapBackend.running && refundRoot.isValidHex(ethSwapIdInput.text, 32)
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
