import QtQuick
import QtQuick.Layouts
import SwapTheme

// Key-value "trust surface" row: a muted micro label over a monospace
// value that elides in the middle — the offer board's detail-pane idiom
// for hashes, addresses and tx ids.
//
// Three states, driven by whether `value` is set and whether `link` was
// handed a resolved explorer URL (callers build that with SwapLinks):
//  - inert: no value — today's "—", no interaction.
//  - copyable: a value with no explorer link (a hashlock, a preimage, the
//    ETH swap ID, or a chain ref on a chain/network with no known
//    explorer) — click copies, label flashes "Copied".
//  - linkable: a value with a resolved explorer link — accent tint,
//    pointing-hand cursor, a trailing "↗", opens externally. Every
//    linkable row ALSO exposes a small "Copy" affix: whether
//    Qt.openUrlExternally is honoured by the Basecamp QML host is
//    unproven, so copy must never be a click away only through a link.
ColumnLayout {
    id: row

    property string label
    property string value
    property string link: ""
    property color valueColor: Theme.textSecondary
    property bool copied: false

    readonly property bool hasValue: value !== ""
    readonly property bool isLinkable: hasValue && link !== ""

    spacing: 1
    Layout.fillWidth: true

    // Pure-QML clipboard: an invisible TextEdit whose selection is copied
    // (same trick as ReceiptCard.qml's "Copy JSON receipt").
    TextEdit {
        id: clipboardHelper
        visible: false
    }

    Timer {
        id: copiedReset
        interval: 1600
        onTriggered: row.copied = false
    }

    function copyValue() {
        clipboardHelper.text = row.value
        clipboardHelper.selectAll()
        clipboardHelper.copy()
        clipboardHelper.text = ""
        row.copied = true
        copiedReset.restart()
    }

    Text {
        text: row.copied ? "Copied" : row.label
        color: row.copied ? Theme.success : Theme.textMuted
        font.pixelSize: Theme.fontMicro
        font.letterSpacing: 0.5
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: 6

        Text {
            Layout.fillWidth: true
            text: row.hasValue ? row.value : "—"
            color: row.isLinkable ? Theme.accent : row.valueColor
            font.pixelSize: Theme.fontCaption
            font.family: Theme.monoFont
            elide: Text.ElideMiddle

            MouseArea {
                anchors.fill: parent
                anchors.margins: -3
                enabled: row.hasValue
                cursorShape: row.hasValue ? Qt.PointingHandCursor : Qt.ArrowCursor
                onClicked: {
                    if (row.isLinkable) {
                        Qt.openUrlExternally(row.link)
                    } else {
                        row.copyValue()
                    }
                }
            }
        }
        Text {
            visible: row.isLinkable
            text: "↗"
            color: Theme.accent
            font.pixelSize: Theme.fontCaption
        }
        Text {
            visible: row.isLinkable
            text: "Copy"
            color: Theme.textMuted
            font.pixelSize: Theme.fontMicro
            font.underline: copyAffixMouse.containsMouse

            MouseArea {
                id: copyAffixMouse
                anchors.fill: parent
                anchors.margins: -4
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: row.copyValue()
            }
        }
    }
}
