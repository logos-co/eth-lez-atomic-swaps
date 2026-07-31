import QtQuick
import QtQuick.Controls
import SwapTheme

// Filled accent call-to-action — the offer board's Accept-button idiom.
Button {
    id: control

    font.pixelSize: Theme.fontNormal
    font.bold: true

    background: Rectangle {
        color: control.enabled
               ? (control.hovered ? Theme.accentHover : Theme.accent)
               : Theme.surfaceLight
        radius: Theme.radiusNormal
    }
    contentItem: Text {
        text: control.text
        color: control.enabled ? Theme.accentForeground : Theme.textMuted
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        font: control.font
    }
}
