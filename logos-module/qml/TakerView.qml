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

    property bool messagingEnabled: swapBackend.nwakuUrl !== ""
    property var discoveredOffers: []
    property bool fetching: false

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
                    if (obj.offers)
                        takerRoot.discoveredOffers = obj.offers
                }
            }

            Text {
                text: "Taker Flow"
                color: Theme.textPrimary
                font.pixelSize: Theme.fontTitle
                font.bold: true
            }
            Text {
                text: "Generate preimage, lock ETH, wait for maker to lock LEZ, then claim LEZ."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }

            // --- Discover Offers (messaging enabled) ---
            Button {
                visible: messagingEnabled
                text: fetching ? "Fetching..." : "Discover Offers"
                enabled: !fetching && !swapBackend.takerRunning
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
                model: discoveredOffers

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
                        onClicked: {
                            // Accept offer — start taker (preimage generated internally)
                            swapBackend.startTaker("")
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

            // No offers message
            Text {
                visible: messagingEnabled && discoveredOffers.length === 0 && !fetching
                text: "No offers found. Click \"Discover Offers\" to search."
                color: Theme.textMuted
                font.pixelSize: Theme.fontSmall
            }

            // Start button (no hashlock input needed — taker generates preimage)
            Button {
                text: swapBackend.takerRunning ? "Running..." : "Start Taker"
                enabled: !swapBackend.takerRunning
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

                onClicked: swapBackend.startTaker("")
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
                    currentStep: swapBackend.takerCurrentStep
                    completedSteps: takerRoot.completedSteps
                }
            }

            // Result
            ResultCard {
                resultJson: swapBackend.takerResultJson
            }
        }
    }
}
