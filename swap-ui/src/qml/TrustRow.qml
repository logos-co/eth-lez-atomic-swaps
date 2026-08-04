import QtQuick
import QtQuick.Layouts
import SwapTheme

// Key-value "trust surface" row: a muted micro label over a monospace
// value that elides in the middle — the offer board's detail-pane idiom
// for hashes, addresses and tx ids.
//
// `link` is a resolved explorer URL built by SwapLinks. Basecamp modules
// cannot open external URLs directly, so both interactions are explicit,
// keyboard-accessible copy buttons: one for the value and one for the URL.
ColumnLayout {
    id: row

    property string label
    property string value
    property string link: ""
    property color valueColor: Theme.textSecondary
    property string copiedKind: ""

    readonly property bool hasValue: value !== ""
    readonly property bool hasExplorerLink: hasValue && link !== ""

    spacing: 1
    Layout.fillWidth: true

    // Pure-QML clipboard: an invisible TextEdit whose selection is copied
    // (same clipboard helper used by ReceiptCard.qml's copy actions).
    TextEdit {
        id: clipboardHelper
        visible: false
    }

    Timer {
        id: copiedReset
        interval: 1600
        onTriggered: row.copiedKind = ""
    }

    function copyText(text, kind) {
        clipboardHelper.text = text
        clipboardHelper.selectAll()
        clipboardHelper.copy()
        clipboardHelper.text = ""
        row.copiedKind = kind
        copiedReset.restart()
    }

    Text {
        text: row.label
        color: Theme.textMuted
        font.pixelSize: Theme.fontMicro
        font.letterSpacing: 0.5
    }

    Text {
        Layout.fillWidth: true
        text: row.hasValue ? row.value : "—"
        color: row.valueColor
        font.pixelSize: Theme.fontCaption
        font.family: Theme.monoFont
        elide: Text.ElideMiddle
    }

    RowLayout {
        visible: row.hasValue
        Layout.fillWidth: true
        spacing: Theme.spacingSmall

        GhostButton {
            text: row.copiedKind === "value" ? "Value copied" : "Copy value"
            accented: false
            Layout.preferredHeight: 26
            font.pixelSize: Theme.fontCaption
            onClicked: row.copyText(row.value, "value")
        }
        GhostButton {
            visible: row.hasExplorerLink
            text: row.copiedKind === "link"
                  ? "Explorer link copied" : "Copy explorer link"
            Layout.preferredHeight: 26
            font.pixelSize: Theme.fontCaption
            onClicked: row.copyText(row.link, "link")
        }
        Item { Layout.fillWidth: true }
    }
}
