import QtQuick
import QtQuick.Layouts
import SwapTheme

// The full credential/parameter form, as an embeddable column.
//
// This was ConfigPanel.qml, a tab of its own sitting next to Setup — two
// entries in the nav for one job, and no answer to "which one do I use?". Setup
// is now the single answer; this is its "Advanced settings" body. Extracted as a
// plain ColumnLayout (not a ScrollView) precisely so it can nest inside
// PageScaffold's scroll surface rather than fighting it.
//
// Every field persists on each keystroke — setConfigValue() writes config.json
// synchronously — so there is deliberately no Save button to forget to press.
ColumnLayout {
    id: form

    property bool anyRunning: swapBackend.makerRunning || swapBackend.takerRunning
                              || swapBackend.autoAcceptRunning
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

    Layout.fillWidth: true
    spacing: Theme.spacingNormal

    // --- Ethereum ---
    SectionHeader {
        label: "Ethereum"
    }

    LabeledField {
        label: "RPC URL"
        text: swapBackend.ethRpcUrl
        onValueEdited: (val) => swapBackend.setConfigValue("eth_rpc_url", val)
        placeholderText: "wss://..."
        hint: "WebSocket (wss://) only. Defaults to PublicNode — public, no signup, "
              + "not Google/Alchemy. Want more privacy? Point it at dRPC, 1RPC, "
              + "POKT/Grove, or your own node."
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("eth_rpc_url")
    }
    LabeledField {
        label: "Private Key"
        text: swapBackend.ethPrivateKey
        onValueEdited: (val) => swapBackend.setConfigValue("eth_private_key", val)
        echoMode: TextInput.Password
        placeholderText: "0x..."
        hint: "Paste an existing key, or generate a fresh one below."
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("eth_private_key")
    }
    GhostButton {
        text: "Generate new key"
        accented: false
        enabled: !form.anyRunning && swapBackend.ready
        Layout.preferredHeight: 34
        font.pixelSize: Theme.fontSmall
        // Auto-fills eth_recipient_address with the derived address
        // too — a taker's recipient is its own address (it locks ETH
        // and later claims/refunds back to itself).
        onClicked: swapBackend.setupGenerateEthKey()
    }
    LabeledField {
        label: "HTLC Contract Address"
        text: swapBackend.ethHtlcAddress
        onValueEdited: (val) => swapBackend.setConfigValue("eth_htlc_address", val)
        placeholderText: "0x..."
        hint: "Pre-filled with the shared Sepolia deployment. When you buy, this comes from the offer you accepted."
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("eth_htlc_address")
    }
    LabeledField {
        label: "Recipient Address"
        text: swapBackend.ethRecipientAddress
        onValueEdited: (val) => swapBackend.setConfigValue("eth_recipient_address", val)
        placeholderText: "0x..."
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("eth_recipient_address")
    }

    // --- LEZ ---
    SectionHeader {
        label: "LEZ"
        Layout.topMargin: Theme.spacingNormal
    }

    LabeledField {
        label: "Sequencer URL"
        text: swapBackend.lezSequencerUrl
        onValueEdited: (val) => swapBackend.setConfigValue("lez_sequencer_url", val)
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("lez_sequencer_url")
    }
    LabeledField {
        label: "Signing Key"
        text: swapBackend.lezSigningKey
        onValueEdited: (val) => swapBackend.setConfigValue("lez_signing_key", val)
        echoMode: TextInput.Password
        placeholderText: "32-byte hex"
        hint: "Paste an existing key, or use the steps above to create and fund a new account."
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("lez_signing_key")
    }
    LabeledField {
        label: "Wallet Home"
        text: swapBackend.lezWalletHome
        onValueEdited: (val) => swapBackend.setConfigValue("lez_wallet_home", val)
        placeholderText: ".scaffold/wallet"
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("lez_wallet_home")
    }
    LabeledField {
        label: "Wallet Account ID"
        text: swapBackend.lezAccountId
        onValueEdited: (val) => swapBackend.setConfigValue("lez_account_id", val)
        placeholderText: "base58"
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("lez_account_id")
    }
    LabeledField {
        label: "HTLC Program ID"
        text: swapBackend.lezHtlcProgramId
        onValueEdited: (val) => swapBackend.setConfigValue("lez_htlc_program_id", val)
        placeholderText: "32-byte hex"
        hint: "Defaults to the canonical compiled-in ID when available. When you buy, this comes from the offer you accepted."
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("lez_htlc_program_id")
    }
    LabeledField {
        label: "Sell to one buyer only (optional)"
        text: swapBackend.lezTakerAccountId
        onValueEdited: (val) => swapBackend.setConfigValue("lez_taker_account_id", val)
        placeholderText: "base58 — leave empty for public offers"
        hint: "Applies when you sell: only this account may take your offers. "
              + "Leave it empty to sell to anyone. Never your own account — a buyer "
              + "always receives on the account derived from their own signing key."
        fieldEnabled: !form.anyRunning
        errorText: form.errorFor("lez_taker_account_id")
    }

    // --- Swap Parameters ---
    SectionHeader {
        label: "Swap Parameters"
        Layout.topMargin: Theme.spacingNormal
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacingNormal

        LabeledField {
            Layout.fillWidth: true
            label: "LEZ Amount"
            text: swapBackend.lezAmount
            onValueEdited: (val) => swapBackend.setConfigValue("lez_amount", val)
            fieldEnabled: !form.anyRunning
            errorText: form.errorFor("lez_amount")
        }
        LabeledField {
            Layout.fillWidth: true
            label: "ETH Amount"
            text: swapBackend.ethAmount
            onValueEdited: (val) => swapBackend.setConfigValue("eth_amount", val)
            fieldEnabled: !form.anyRunning
            errorText: form.errorFor("eth_amount")
        }
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacingNormal

        LabeledField {
            Layout.fillWidth: true
            label: "LEZ Timelock (min)"
            text: swapBackend.lezTimelockMinutes
            onValueEdited: (val) => swapBackend.setConfigValue("lez_timelock_minutes", val)
            fieldEnabled: !form.anyRunning
            errorText: form.errorFor("lez_timelock_minutes")
        }
        LabeledField {
            Layout.fillWidth: true
            label: "ETH Timelock (min)"
            text: swapBackend.ethTimelockMinutes
            onValueEdited: (val) => swapBackend.setConfigValue("eth_timelock_minutes", val)
            fieldEnabled: !form.anyRunning
            errorText: form.errorFor("eth_timelock_minutes")
        }
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacingNormal

        LabeledField {
            Layout.fillWidth: true
            label: "Poll Interval (ms)"
            text: swapBackend.pollIntervalMs
            onValueEdited: (val) => swapBackend.setConfigValue("poll_interval_ms", val)
            fieldEnabled: !form.anyRunning
            errorText: form.errorFor("poll_interval_ms")
        }
        Item { Layout.fillWidth: true }
    }

    // --- Developer ---
    // These read .env / .env.taker relative to the process working directory,
    // walking up six parents to find them. That works in a repo checkout and
    // nowhere else — in a Basecamp bundle install both silently fail. They used
    // to be the first thing on the Config screen; they belong here, labelled,
    // at the bottom.
    SectionHeader {
        label: "Developer"
        Layout.topMargin: Theme.spacingNormal
    }
    Text {
        Layout.fillWidth: true
        text: "Loads credentials from a .env file next to the source checkout. "
              + "Only works when running from the repo."
        color: Theme.textMuted
        font.pixelSize: Theme.fontCaption
        horizontalAlignment: Text.AlignLeft
        wrapMode: Text.WordWrap
    }
    // These two keep the protocol role names on purpose. Everywhere a user
    // talks about money the app says seller/buyer, but these buttons load
    // `.env` / `.env.taker` from a source checkout and pass the literal role
    // string to the backend — renaming them would stop them matching the files
    // and the role argument a developer is actually looking at.
    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacingNormal

        GhostButton {
            text: "Load Maker Env"
            accented: false
            enabled: !form.anyRunning && !form.anyLoading && swapBackend.ready
            Layout.fillWidth: true
            Layout.preferredHeight: 34
            font.pixelSize: Theme.fontSmall
            onClicked: swapBackend.loadEnvFile(".env", "maker")
        }
        GhostButton {
            text: "Load Taker Env"
            accented: false
            enabled: !form.anyRunning && !form.anyLoading && swapBackend.ready
            Layout.fillWidth: true
            Layout.preferredHeight: 34
            font.pixelSize: Theme.fontSmall
            onClicked: swapBackend.loadEnvFile(".env.taker", "taker")
        }
    }
}
