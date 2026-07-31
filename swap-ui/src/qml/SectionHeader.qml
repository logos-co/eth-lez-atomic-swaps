import QtQuick
import QtQuick.Layouts
import SwapTheme

// Uppercase micro-label section header — the offer board's column-header
// vocabulary (small, bold, letter-spaced, muted), with an optional
// hairline rule underneath.
Item {
    id: header

    property string label
    property bool hairline: true
    // Optional trailing accent text (e.g. a count), rendered muted.
    property string detail: ""

    Layout.fillWidth: true
    implicitHeight: labelText.implicitHeight
        + (hairline ? Theme.spacingSmall : 0)

    Text {
        id: labelText
        text: header.label.toUpperCase()
        color: Theme.textMuted
        font.pixelSize: Theme.fontCaption
        font.bold: true
        font.letterSpacing: 1
    }

    Text {
        visible: header.detail !== ""
        anchors.baseline: labelText.baseline
        anchors.left: labelText.right
        anchors.leftMargin: Theme.spacingSmall
        text: header.detail
        color: Theme.textMuted
        font.pixelSize: Theme.fontCaption
    }

    Rectangle {
        visible: header.hairline
        anchors.bottom: parent.bottom
        width: parent.width
        height: 1
        color: Theme.border
    }
}
