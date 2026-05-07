import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import "."

ScrollView {
    id: configRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    property bool anyRunning: swapBackend.makerRunning || swapBackend.takerRunning || swapBackend.autoAcceptRunning
    property bool anyLoading: swapBackend.balancesLoading || swapBackend.messagingLoading
                              || swapBackend.offersLoading || swapBackend.publishingLoading
                              || swapBackend.refundsLoading
    property var validationErrors: {
        try { return JSON.parse(swapBackend.validationErrorsJson || "{}") }
        catch (e) { return {} }
    }

    function errorFor(key) {
        return validationErrors && validationErrors[key] ? validationErrors[key] : ""
    }

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
                text: "Edit values below before starting a swap."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: Theme.spacingNormal

                Button {
                    text: "Load Maker Env"
                    enabled: !configRoot.anyRunning && !configRoot.anyLoading && swapBackend.ready
                    Layout.fillWidth: true
                    Layout.preferredHeight: 38
                    contentItem: Text {
                        text: parent.text
                        color: parent.enabled ? Theme.textPrimary : Theme.textMuted
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                        font.pixelSize: Theme.fontSmall
                        font.bold: true
                    }
                    background: Rectangle {
                        radius: Theme.radiusSmall
                        color: parent.enabled && parent.hovered ? Theme.surfaceLight : Theme.surface
                        border.color: parent.enabled ? Theme.accent : Theme.border
                        border.width: 1
                    }
                    onClicked: swapBackend.loadEnvFile(".env", "maker")
                }
                Button {
                    text: "Load Taker Env"
                    enabled: !configRoot.anyRunning && !configRoot.anyLoading && swapBackend.ready
                    Layout.fillWidth: true
                    Layout.preferredHeight: 38
                    contentItem: Text {
                        text: parent.text
                        color: parent.enabled ? Theme.textPrimary : Theme.textMuted
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                        font.pixelSize: Theme.fontSmall
                        font.bold: true
                    }
                    background: Rectangle {
                        radius: Theme.radiusSmall
                        color: parent.enabled && parent.hovered ? Theme.surfaceLight : Theme.surface
                        border.color: parent.enabled ? Theme.accent : Theme.border
                        border.width: 1
                    }
                    onClicked: swapBackend.loadEnvFile(".env.taker", "taker")
                }
            }

            // --- Ethereum ---
            SectionHeader { label: "Ethereum" }

            ConfigField {
                label: "RPC URL"
                text: swapBackend.ethRpcUrl
                onValueEdited: (val) => swapBackend.setConfigValue("eth_rpc_url", val)
                placeholderText: "wss://..."
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("eth_rpc_url")
            }
            ConfigField {
                label: "Private Key"
                text: swapBackend.ethPrivateKey
                onValueEdited: (val) => swapBackend.setConfigValue("eth_private_key", val)
                echoMode: TextInput.Password
                placeholderText: "0x..."
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("eth_private_key")
            }
            ConfigField {
                label: "HTLC Contract Address"
                text: swapBackend.ethHtlcAddress
                onValueEdited: (val) => swapBackend.setConfigValue("eth_htlc_address", val)
                placeholderText: "0x..."
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("eth_htlc_address")
            }
            ConfigField {
                label: "Recipient Address"
                text: swapBackend.ethRecipientAddress
                onValueEdited: (val) => swapBackend.setConfigValue("eth_recipient_address", val)
                placeholderText: "0x..."
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("eth_recipient_address")
            }

            // --- LEZ ---
            SectionHeader { label: "LEZ" }

            ConfigField {
                label: "Sequencer URL"
                text: swapBackend.lezSequencerUrl
                onValueEdited: (val) => swapBackend.setConfigValue("lez_sequencer_url", val)
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("lez_sequencer_url")
            }
            ConfigField {
                label: "Signing Key"
                text: swapBackend.lezSigningKey
                onValueEdited: (val) => swapBackend.setConfigValue("lez_signing_key", val)
                echoMode: TextInput.Password
                placeholderText: "32-byte hex"
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("lez_signing_key")
            }
            ConfigField {
                label: "Wallet Home"
                text: swapBackend.lezWalletHome
                onValueEdited: (val) => swapBackend.setConfigValue("lez_wallet_home", val)
                placeholderText: ".scaffold/wallet"
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("lez_wallet_home")
            }
            ConfigField {
                label: "Wallet Account ID"
                text: swapBackend.lezAccountId
                onValueEdited: (val) => swapBackend.setConfigValue("lez_account_id", val)
                placeholderText: "base58"
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("lez_account_id")
            }
            ConfigField {
                label: "HTLC Program ID"
                text: swapBackend.lezHtlcProgramId
                onValueEdited: (val) => swapBackend.setConfigValue("lez_htlc_program_id", val)
                placeholderText: "32-byte hex"
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("lez_htlc_program_id")
            }
            ConfigField {
                label: "Taker Account ID"
                text: swapBackend.lezTakerAccountId
                onValueEdited: (val) => swapBackend.setConfigValue("lez_taker_account_id", val)
                placeholderText: "base58"
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("lez_taker_account_id")
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
                    onValueEdited: (val) => swapBackend.setConfigValue("lez_amount", val)
                    fieldEnabled: !configRoot.anyRunning
                    errorText: configRoot.errorFor("lez_amount")
                }
                ConfigField {
                    Layout.fillWidth: true
                    label: "ETH Amount"
                    text: swapBackend.ethAmount
                    onValueEdited: (val) => swapBackend.setConfigValue("eth_amount", val)
                    fieldEnabled: !configRoot.anyRunning
                    errorText: configRoot.errorFor("eth_amount")
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: Theme.spacingNormal

                ConfigField {
                    Layout.fillWidth: true
                    label: "LEZ Timelock (min)"
                    text: swapBackend.lezTimelockMinutes
                    onValueEdited: (val) => swapBackend.setConfigValue("lez_timelock_minutes", val)
                    fieldEnabled: !configRoot.anyRunning
                    errorText: configRoot.errorFor("lez_timelock_minutes")
                }
                ConfigField {
                    Layout.fillWidth: true
                    label: "ETH Timelock (min)"
                    text: swapBackend.ethTimelockMinutes
                    onValueEdited: (val) => swapBackend.setConfigValue("eth_timelock_minutes", val)
                    fieldEnabled: !configRoot.anyRunning
                    errorText: configRoot.errorFor("eth_timelock_minutes")
                }
            }

            ConfigField {
                Layout.fillWidth: true
                label: "Poll Interval (ms)"
                text: swapBackend.pollIntervalMs
                onValueEdited: (val) => swapBackend.setConfigValue("poll_interval_ms", val)
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("poll_interval_ms")
            }

            // --- Messaging ---
            SectionHeader { label: "Messaging" }

            ConfigField {
                label: "Bootstrap Multiaddr"
                text: swapBackend.wakuBootstrapMultiaddr
                onValueEdited: (val) => swapBackend.setConfigValue("waku_bootstrap_multiaddr", val)
                placeholderText: "/ip4/127.0.0.1/tcp/60010/p2p/..."
                fieldEnabled: !configRoot.anyRunning
                errorText: configRoot.errorFor("waku_bootstrap_multiaddr")
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
        id: field

        property alias label: labelText.text
        property alias text: input.text
        property alias echoMode: input.echoMode
        property alias placeholderText: input.placeholderText
        property bool fieldEnabled: true
        property string errorText: ""
        signal valueEdited(string val)

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
                border.color: field.errorText !== "" ? Theme.error : (input.activeFocus ? Theme.accent : Theme.border)
                border.width: 1
                radius: Theme.radiusSmall
                opacity: input.enabled ? 1.0 : 0.5
            }
            onTextChanged: field.valueEdited(input.text)
        }
        Text {
            visible: field.errorText !== ""
            text: field.errorText
            color: Theme.error
            font.pixelSize: 11
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
    }
}
