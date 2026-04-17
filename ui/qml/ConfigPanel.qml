import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: configRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    Flickable {
        contentHeight: col.implicitHeight + Theme.spacingXLarge * 2
        boundsBehavior: Flickable.StopAtBounds

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
                text: "Configuration"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            Text {
                text: "Pre-filled from .env. Edit values below before starting a swap."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }

            // --- Ethereum ---
            SectionHeader { label: "Ethereum" }

            ConfigField {
                label: "RPC URL"
                text: swapBackend.ethRpcUrl
                onTextEdited: (val) => swapBackend.ethRpcUrl = val
                placeholderText: "wss://..."
                fieldEnabled: !swapBackend.running
            }
            ConfigField {
                label: "Private Key"
                text: swapBackend.ethPrivateKey
                onTextEdited: (val) => swapBackend.ethPrivateKey = val
                echoMode: TextInput.Password
                placeholderText: "0x..."
                fieldEnabled: !swapBackend.running
            }
            ConfigField {
                label: "HTLC Contract Address"
                text: swapBackend.ethHtlcAddress
                onTextEdited: (val) => swapBackend.ethHtlcAddress = val
                placeholderText: "0x..."
                fieldEnabled: !swapBackend.running
            }
            ConfigField {
                label: "Recipient Address"
                text: swapBackend.ethRecipientAddress
                onTextEdited: (val) => swapBackend.ethRecipientAddress = val
                placeholderText: "0x..."
                fieldEnabled: !swapBackend.running
            }

            // --- LEZ ---
            SectionHeader { label: "LEZ" }

            ConfigField {
                label: "Sequencer URL"
                text: swapBackend.lezSequencerUrl
                onTextEdited: (val) => swapBackend.lezSequencerUrl = val
                fieldEnabled: !swapBackend.running
            }
            ConfigField {
                label: "Signing Key"
                text: swapBackend.lezSigningKey
                onTextEdited: (val) => swapBackend.lezSigningKey = val
                echoMode: TextInput.Password
                placeholderText: "32-byte hex"
                fieldEnabled: !swapBackend.running
            }
            ConfigField {
                label: "HTLC Program ID"
                text: swapBackend.lezHtlcProgramId
                onTextEdited: (val) => swapBackend.lezHtlcProgramId = val
                placeholderText: "32-byte hex"
                fieldEnabled: !swapBackend.running
            }
            ConfigField {
                label: "Taker Account ID"
                text: swapBackend.lezTakerAccountId
                onTextEdited: (val) => swapBackend.lezTakerAccountId = val
                placeholderText: "base58"
                fieldEnabled: !swapBackend.running
            }

            // --- Swap Parameters ---
            SectionHeader { label: "Swap Parameters" }

            RowLayout {
                Layout.fillWidth: true
                spacing: Theme.spacingNormal

                ConfigField {
                    Layout.fillWidth: true
                    label: "LEZ Amount"
                    text: swapBackend.lezAmount
                    onTextEdited: (val) => swapBackend.lezAmount = val
                    fieldEnabled: !swapBackend.running
                }
                ConfigField {
                    Layout.fillWidth: true
                    label: "ETH Amount"
                    text: swapBackend.ethAmount
                    onTextEdited: (val) => swapBackend.ethAmount = val
                    fieldEnabled: !swapBackend.running
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: Theme.spacingNormal

                ConfigField {
                    Layout.fillWidth: true
                    label: "LEZ Timelock (min)"
                    text: swapBackend.lezTimelockMinutes
                    onTextEdited: (val) => swapBackend.lezTimelockMinutes = val
                    fieldEnabled: !swapBackend.running
                }
                ConfigField {
                    Layout.fillWidth: true
                    label: "ETH Timelock (min)"
                    text: swapBackend.ethTimelockMinutes
                    onTextEdited: (val) => swapBackend.ethTimelockMinutes = val
                    fieldEnabled: !swapBackend.running
                }
            }

            ConfigField {
                Layout.fillWidth: true
                label: "Poll Interval (ms)"
                text: swapBackend.pollIntervalMs
                onTextEdited: (val) => swapBackend.pollIntervalMs = val
                fieldEnabled: !swapBackend.running
            }

            // --- Messaging ---
            SectionHeader { label: "Messaging" }

            ConfigField {
                label: "Bootstrap Multiaddr"
                text: swapBackend.wakuBootstrapMultiaddr
                onTextEdited: (val) => swapBackend.wakuBootstrapMultiaddr = val
                placeholderText: "/ip4/127.0.0.1/tcp/60010/p2p/..."
                fieldEnabled: !swapBackend.running
            }
        }
    }

    // --- Inline sub-components ---
    component SectionHeader: Rectangle {
        property string label
        Layout.fillWidth: true
        Layout.topMargin: Theme.spacingLarge
        height: 36
        color: "transparent"

        Text {
            text: parent.label
            color: Theme.accent
            font.pixelSize: Theme.fontLarge
            font.bold: true
            anchors.verticalCenter: parent.verticalCenter
        }
        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.border
        }
    }

    component ConfigField: ColumnLayout {
        id: fieldRoot
        property alias label: labelText.text
        property alias text: input.text
        property alias echoMode: input.echoMode
        property alias placeholderText: input.placeholderText
        property bool fieldEnabled: true
        signal textEdited(string val)

        spacing: 4

        Text {
            id: labelText
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
        }
        TextField {
            id: input
            enabled: fieldEnabled
            Layout.fillWidth: true
            Layout.preferredHeight: Theme.inputHeight
            leftPadding: 12
            rightPadding: 12
            topPadding: 8
            bottomPadding: 8
            color: Theme.textPrimary
            font.pixelSize: Theme.fontNormal
            selectByMouse: true
            background: Rectangle {
                color: Theme.inputBackground
                border.color: input.activeFocus ? Theme.accent : Theme.border
                border.width: 1
                radius: Theme.radiusSmall
                opacity: input.enabled ? 1.0 : 0.5
            }
            onTextChanged: fieldRoot.textEdited(input.text)
        }
    }
}
