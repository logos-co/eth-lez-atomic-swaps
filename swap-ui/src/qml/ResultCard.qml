import QtQuick
import QtQuick.Layouts
import SwapTheme

Rectangle {
    id: card

    required property string resultJson
    // "taker" | "maker" — the backend's `eth_tx` is role-dependent
    // (src/swap/types.rs): the maker's ETH claim *tx hash* but the
    // taker's ETH lock *swap ID*. Label it accordingly.
    property string role: ""

    readonly property var parsed: {
        if (!resultJson) return null
        try { return JSON.parse(resultJson) }
        catch (e) { return { error: resultJson } }
    }

    readonly property bool isError: parsed && parsed.error !== undefined
    readonly property bool isCompleted: parsed && parsed.status === "completed"
    readonly property bool isRefunded: parsed && parsed.status === "refunded"
    readonly property bool visible_: parsed !== null

    readonly property color tone: isError ? Theme.error
                                          : isCompleted ? Theme.success
                                                        : Theme.warning

    visible: visible_
    Layout.fillWidth: true
    implicitHeight: resultCol.implicitHeight + Theme.spacingNormal * 2
    radius: Theme.radiusNormal
    color: Theme.surface
    border.color: tone
    border.width: 1

    ColumnLayout {
        id: resultCol
        anchors {
            fill: parent
            margins: Theme.spacingNormal
        }
        spacing: Theme.spacingSmall

        // Status header — dot + uppercase micro-label (chip idiom)
        RowLayout {
            spacing: 6

            Rectangle {
                width: 6; height: 6; radius: 3
                color: card.tone
            }
            Text {
                text: card.isError ? "ERROR"
                                   : card.isCompleted ? "SWAP COMPLETED"
                                                      : card.isRefunded ? "SWAP REFUNDED"
                                                                        : "RESULT"
                color: card.tone
                font.pixelSize: Theme.fontCaption
                font.bold: true
                font.letterSpacing: 1
            }
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

        // Completed details — trust surface
        Repeater {
            model: card.isCompleted ? [
                { label: "Hashlock", value: card.parsed.hashlock || "" },
                { label: "Preimage", value: card.parsed.preimage || "" },
                { label: card.role === "taker" ? "ETH swap ID (lock)"
                       : card.role === "maker" ? "ETH claim tx"
                                               : "ETH tx",
                  value: card.parsed.eth_tx || "" },
                { label: card.role === "taker" ? "LEZ claim tx"
                       : card.role === "maker" ? "LEZ lock tx"
                                               : "LEZ tx",
                  value: card.parsed.lez_tx || "" },
            ] : []

            TrustRow {
                label: modelData.label
                value: modelData.value
            }
        }

        // Refunded details — trust surface
        Repeater {
            model: card.isRefunded ? [
                { label: "ETH Refund", value: card.parsed.eth_refund_tx || "n/a" },
                { label: "LEZ Refund", value: card.parsed.lez_refund_tx || "n/a" },
            ] : []

            TrustRow {
                label: modelData.label
                value: modelData.value
            }
        }
    }
}
