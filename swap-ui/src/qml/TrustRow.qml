import QtQuick
import QtQuick.Layouts
import SwapTheme

// Key-value "trust surface" row: a muted micro label over a monospace
// value that elides in the middle — the offer board's detail-pane idiom
// for hashes, addresses and tx ids.
ColumnLayout {
    id: row

    property string label
    property string value
    property color valueColor: Theme.textSecondary

    spacing: 1
    Layout.fillWidth: true

    Text {
        text: row.label
        color: Theme.textMuted
        font.pixelSize: Theme.fontMicro
        font.letterSpacing: 0.5
    }
    Text {
        Layout.fillWidth: true
        text: row.value !== "" ? row.value : "—"
        color: row.valueColor
        font.pixelSize: Theme.fontCaption
        font.family: Theme.monoFont
        elide: Text.ElideMiddle
    }
}
