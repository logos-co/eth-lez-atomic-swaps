import QtQuick
import QtQuick.Layouts

Rectangle {
    id: card

    required property string resultJson

    readonly property var parsed: {
        if (!resultJson) return null
        try { return JSON.parse(resultJson) }
        catch (e) { return { error: resultJson } }
    }

    readonly property bool isError: parsed && parsed.error !== undefined
    readonly property bool isCompleted: parsed && parsed.status === "completed"
    readonly property bool isRefunded: parsed && parsed.status === "refunded"
    readonly property bool visible_: parsed !== null

    visible: visible_
    Layout.fillWidth: true
    implicitHeight: resultCol.implicitHeight + Theme.spacingNormal * 2
    radius: Theme.radiusNormal
    color: isError ? "#2d1520" : isCompleted ? "#152d20" : Theme.surfaceLight
    border.color: isError ? Theme.error : isCompleted ? Theme.success : Theme.warning
    border.width: 1

    ColumnLayout {
        id: resultCol
        anchors {
            fill: parent
            margins: Theme.spacingNormal
        }
        spacing: Theme.spacingSmall

        Text {
            text: card.isError ? "Error" : card.isCompleted ? "Swap Completed" : card.isRefunded ? "Swap Refunded" : "Result"
            color: card.isError ? Theme.error : card.isCompleted ? Theme.success : Theme.warning
            font.pixelSize: Theme.fontLarge
            font.bold: true
        }

        // Error message
        Text {
            visible: card.isError
            text: card.parsed ? (card.parsed.error || "") : ""
            color: Theme.textPrimary
            font.pixelSize: Theme.fontSmall
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }

        // Completed details
        Repeater {
            model: card.isCompleted ? [
                { label: "Hashlock", value: card.parsed.hashlock || "" },
                { label: "Preimage", value: card.parsed.preimage || "" },
                { label: "ETH Tx", value: card.parsed.eth_tx || "" },
                { label: "LEZ Tx", value: card.parsed.lez_tx || "" },
            ] : []

            RowLayout {
                required property var modelData
                Layout.fillWidth: true
                spacing: Theme.spacingSmall

                Text {
                    text: modelData.label + ":"
                    color: Theme.textSecondary
                    font.pixelSize: Theme.fontSmall
                    Layout.preferredWidth: 80
                }
                Text {
                    text: modelData.value
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontSmall
                    font.family: "Menlo, Courier New"
                    elide: Text.ElideMiddle
                    Layout.fillWidth: true
                }
            }
        }

        // Refunded details
        Repeater {
            model: card.isRefunded ? [
                { label: "ETH Refund", value: card.parsed.eth_refund_tx || "n/a" },
                { label: "LEZ Refund", value: card.parsed.lez_refund_tx || "n/a" },
            ] : []

            RowLayout {
                required property var modelData
                Layout.fillWidth: true
                spacing: Theme.spacingSmall

                Text {
                    text: modelData.label + ":"
                    color: Theme.textSecondary
                    font.pixelSize: Theme.fontSmall
                    Layout.preferredWidth: 90
                }
                Text {
                    text: modelData.value
                    color: Theme.textPrimary
                    font.pixelSize: Theme.fontSmall
                    font.family: "Menlo, Courier New"
                    elide: Text.ElideMiddle
                    Layout.fillWidth: true
                }
            }
        }
    }
}
