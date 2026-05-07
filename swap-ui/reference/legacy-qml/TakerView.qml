import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: takerRoot
    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    property var takerSteps: [
        { name: "PreimageGenerated", label: "Generate Preimage" },
        { name: "EthLocked",         label: "Lock ETH" },
        { name: "LezLockDetected",   label: "Wait for LEZ Lock" },
        { name: "LezClaimed",        label: "Claim LEZ" },
    ]

    property var completedSteps: {
        var done = []
        var steps = swapBackend.takerProgressSteps
        for (var i = 0; i < steps.length; i++) {
            if (done.indexOf(steps[i]) < 0)
                done.push(steps[i])
        }
        return done
    }

    property var discoveredOffers: []
    property bool fetching: false
    property var pendingOffer: null
    property var acceptedOffer: null
    property bool swapCompleted: false

    // Convert wei to ETH numeric string (for config fields, not display)
    function weiToEthValue(wei) {
        var n = Number(wei)
        if (isNaN(n) || n === 0) return "0"
        var eth = n / 1e18
        return eth.toString()
    }

    function weiToEth(wei) {
        var n = Number(wei)
        if (isNaN(n) || n === 0) return "0 ETH"
        var eth = n / 1e18
        if (eth >= 0.001) return eth.toFixed(6).replace(/\.?0+$/, '') + " ETH"
        // Show in Gwei for small amounts
        var gwei = n / 1e9
        if (gwei >= 1) return gwei.toFixed(4).replace(/\.?0+$/, '') + " Gwei"
        return wei + " wei"
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

    // timelockSec is an absolute Unix timestamp in seconds
    function expiresIn(timelockSec) {
        if (!timelockSec) return ""
        var diff = timelockSec - Math.floor(Date.now() / 1000)
        if (diff <= 0) return "expired"
        var min = Math.floor(diff / 60)
        if (min < 60) return min + "m"
        var hr = Math.floor(min / 60)
        return hr + "h " + (min % 60) + "m"
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

            Connections {
                target: swapBackend
                function onOffersFetched(offersJson) {
                    takerRoot.fetching = false
                    var obj = JSON.parse(offersJson)
                    if (obj.offers && obj.offers.length > 0) {
                        // Merge new offers with existing (relay drains are destructive)
                        var merged = takerRoot.discoveredOffers.slice()
                        var seen = {}
                        for (var i = 0; i < merged.length; i++)
                            seen[merged[i].maker_eth_address + ":" + merged[i].lez_amount + ":" + merged[i].eth_amount] = true
                        for (var j = 0; j < obj.offers.length; j++) {
                            var o = obj.offers[j]
                            var key = o.maker_eth_address + ":" + o.lez_amount + ":" + o.eth_amount
                            if (!seen[key]) {
                                merged.push(o)
                                seen[key] = true
                            }
                        }
                        takerRoot.discoveredOffers = merged
                    }
                }
                function onTakerRunningChanged() {
                    if (!swapBackend.takerRunning && takerRoot.acceptedOffer !== null) {
                        takerRoot.swapCompleted = true
                        takerRoot.acceptedOffer = null
                    }
                }
            }

            Text {
                text: "Buy LEZ"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            Text {
                visible: !swapBackend.takerRunning && !takerRoot.swapCompleted
                text: "Browse available offers and click one to start a swap."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // --- Discover Offers ---
            Button {
                visible: !swapBackend.takerRunning && !takerRoot.swapCompleted
                text: fetching ? "Fetching..." : "Discover Offers"
                enabled: !fetching && !swapBackend.takerRunning && swapBackend.messagingConnected
                Layout.fillWidth: true
                Layout.preferredHeight: 42
                font.pixelSize: Theme.fontNormal
                font.bold: true

                background: Rectangle {
                    color: parent.enabled
                           ? (parent.hovered ? Qt.darker(Theme.surface, 1.1) : Theme.surface)
                           : Theme.surfaceLight
                    border.color: Theme.accent
                    border.width: 1
                    radius: Theme.radiusNormal
                }
                contentItem: Text {
                    text: parent.text
                    color: parent.enabled ? Theme.accent : Theme.textMuted
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    font: parent.font
                }

                onClicked: {
                    takerRoot.fetching = true
                    swapBackend.fetchOffers()
                }
            }

            // Offer list
            Repeater {
                model: !swapBackend.takerRunning && !takerRoot.swapCompleted ? discoveredOffers : []

                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: offerItemCol.implicitHeight + Theme.spacingNormal * 2
                    color: offerMouse.containsMouse ? Qt.darker(Theme.surface, 1.05) : Theme.surface
                    border.color: offerMouse.containsMouse ? Theme.accent : Theme.border
                    border.width: 1
                    radius: Theme.radiusSmall

                    MouseArea {
                        id: offerMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        enabled: !swapBackend.takerRunning && takerRoot.pendingOffer === null
                        onClicked: {
                            takerRoot.pendingOffer = modelData
                        }
                    }

                    ColumnLayout {
                        id: offerItemCol
                        anchors {
                            fill: parent
                            margins: Theme.spacingNormal
                        }
                        spacing: 6

                        // Row 1: amounts + time
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacingNormal

                            Text {
                                text: modelData.lez_amount + " LEZ"
                                color: Theme.textPrimary
                                font.pixelSize: Theme.fontNormal
                                font.bold: true
                            }
                            Text {
                                text: "\u21C4"
                                color: Theme.textMuted
                                font.pixelSize: Theme.fontNormal
                            }
                            Text {
                                text: takerRoot.weiToEth(modelData.eth_amount)
                                color: Theme.textPrimary
                                font.pixelSize: Theme.fontNormal
                                font.bold: true
                            }
                            Item { Layout.fillWidth: true }
                            Text {
                                text: takerRoot.timeAgo(modelData.timestamp_ms)
                                color: Theme.textMuted
                                font.pixelSize: 11
                            }
                        }

                        // Row 2: maker address + timelocks
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacingNormal

                            Text {
                                text: "Maker: " + modelData.maker_eth_address.substring(0, 10) + "..."
                                color: Theme.textSecondary
                                font.pixelSize: 12
                                font.family: "Menlo, Courier New"
                            }
                            Item { Layout.fillWidth: true }
                            Text {
                                text: "LEZ " + takerRoot.expiresIn(modelData.lez_timelock)
                                      + " / ETH " + takerRoot.expiresIn(modelData.eth_timelock)
                                color: Theme.textMuted
                                font.pixelSize: 11
                            }
                        }
                    }
                }
            }

            // Connecting hint
            Text {
                visible: !swapBackend.messagingConnected && swapBackend.wakuBootstrapMultiaddr !== "" && !swapBackend.takerRunning && !takerRoot.swapCompleted
                text: "Connecting to messaging network..."
                color: Theme.warning
                font.pixelSize: Theme.fontSmall
            }

            // No offers message
            Text {
                visible: discoveredOffers.length === 0 && !fetching && !swapBackend.takerRunning && !takerRoot.swapCompleted && takerRoot.pendingOffer === null && swapBackend.messagingConnected
                text: "No offers found. Click \"Discover Offers\" to search."
                color: Theme.textMuted
                font.pixelSize: Theme.fontSmall
            }

            // --- Confirm Purchase Card ---
            Rectangle {
                visible: takerRoot.pendingOffer !== null && !swapBackend.takerRunning
                Layout.fillWidth: true
                implicitHeight: confirmCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.accent
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: confirmCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: 8

                    Text {
                        text: takerRoot.pendingOffer
                              ? "Buy " + takerRoot.pendingOffer.lez_amount + " LEZ for " + takerRoot.weiToEth(takerRoot.pendingOffer.eth_amount) + "?"
                              : ""
                        color: Theme.textPrimary
                        font.pixelSize: Theme.fontNormal
                        font.bold: true
                    }
                    Text {
                        text: takerRoot.pendingOffer
                              ? "from " + takerRoot.pendingOffer.maker_eth_address.substring(0, 6) + "..." + takerRoot.pendingOffer.maker_eth_address.substring(takerRoot.pendingOffer.maker_eth_address.length - 4)
                              : ""
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Theme.spacingNormal

                        Button {
                            text: "Buy"
                            Layout.fillWidth: true
                            Layout.preferredHeight: 40
                            font.pixelSize: Theme.fontNormal
                            font.bold: true

                            background: Rectangle {
                                color: parent.hovered ? Theme.accentHover : Theme.accent
                                radius: Theme.radiusNormal
                            }
                            contentItem: Text {
                                text: parent.text
                                color: "#ffffff"
                                horizontalAlignment: Text.AlignHCenter
                                verticalAlignment: Text.AlignVCenter
                                font: parent.font
                            }

                            onClicked: {
                                var offer = takerRoot.pendingOffer
                                swapBackend.ethRecipientAddress = offer.maker_eth_address
                                swapBackend.lezAmount = String(offer.lez_amount)
                                swapBackend.ethAmount = takerRoot.weiToEthValue(offer.eth_amount)
                                swapBackend.ethHtlcAddress = offer.eth_htlc_address
                                swapBackend.lezHtlcProgramId = offer.lez_htlc_program_id
                                swapBackend.lezTakerAccountId = offer.maker_lez_account
                                takerRoot.acceptedOffer = offer
                                takerRoot.pendingOffer = null
                                swapBackend.startTaker("")
                            }
                        }

                        Button {
                            text: "Cancel"
                            Layout.fillWidth: true
                            Layout.preferredHeight: 40
                            font.pixelSize: Theme.fontNormal

                            background: Rectangle {
                                color: parent.hovered ? Qt.darker(Theme.surface, 1.1) : Theme.surface
                                border.color: Theme.border
                                border.width: 1
                                radius: Theme.radiusNormal
                            }
                            contentItem: Text {
                                text: parent.text
                                color: Theme.textPrimary
                                horizontalAlignment: Text.AlignHCenter
                                verticalAlignment: Text.AlignVCenter
                                font: parent.font
                            }

                            onClicked: {
                                takerRoot.pendingOffer = null
                            }
                        }
                    }
                }
            }

            // --- Active Swap: Accepted Offer + Progress ---
            Rectangle {
                visible: swapBackend.takerRunning && takerRoot.acceptedOffer !== null
                Layout.fillWidth: true
                implicitHeight: activeCol.implicitHeight + Theme.spacingNormal * 2
                color: Theme.surface
                border.color: Theme.accent
                border.width: 1
                radius: Theme.radiusNormal

                ColumnLayout {
                    id: activeCol
                    anchors {
                        fill: parent
                        margins: Theme.spacingNormal
                    }
                    spacing: 6

                    Text {
                        text: takerRoot.acceptedOffer
                              ? "Buying " + takerRoot.acceptedOffer.lez_amount + " LEZ for " + takerRoot.weiToEth(takerRoot.acceptedOffer.eth_amount)
                              : ""
                        color: Theme.accent
                        font.pixelSize: Theme.fontNormal
                        font.bold: true
                    }
                    Text {
                        text: takerRoot.acceptedOffer
                              ? "from " + takerRoot.acceptedOffer.maker_eth_address.substring(0, 6) + "..." + takerRoot.acceptedOffer.maker_eth_address.substring(takerRoot.acceptedOffer.maker_eth_address.length - 4)
                              : ""
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontSmall
                    }
                }
            }

            // Progress (only during active swap)
            Rectangle {
                visible: swapBackend.takerRunning
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
                    currentStep: swapBackend.takerCurrentStep
                    completedSteps: takerRoot.completedSteps
                }
            }

            // Result
            ResultCard {
                resultJson: swapBackend.takerResultJson
            }

            // --- Browse More Offers (post-swap) ---
            Button {
                visible: takerRoot.swapCompleted && !swapBackend.takerRunning
                text: "Browse More Offers"
                Layout.fillWidth: true
                Layout.preferredHeight: 42
                font.pixelSize: Theme.fontNormal
                font.bold: true

                background: Rectangle {
                    color: parent.hovered ? Qt.darker(Theme.surface, 1.1) : Theme.surface
                    border.color: Theme.accent
                    border.width: 1
                    radius: Theme.radiusNormal
                }
                contentItem: Text {
                    text: parent.text
                    color: Theme.accent
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    font: parent.font
                }

                onClicked: {
                    takerRoot.swapCompleted = false
                    takerRoot.pendingOffer = null
                    takerRoot.acceptedOffer = null
                    takerRoot.discoveredOffers = []
                }
            }
        }
    }
}
