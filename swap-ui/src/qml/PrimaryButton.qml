import QtQuick
import QtQuick.Controls
import SwapTheme

// Filled accent call-to-action — the offer board's Accept-button idiom.
//
// Disabled is an OUTLINE, not a filled grey slab. Filling it with surfaceLight
// put a #2A2A2A block on a #242424 card — a 4% step, which read as an empty
// container rather than an unavailable button. At full width (the Sell tab's
// "Go Live & Publish Offer" ran to 1730px) it looked like a rendering fault.
// An outline still reads as a button; it just visibly has nothing behind it.
Button {
    id: control

    font.pixelSize: Theme.fontNormal
    font.bold: true

    background: Rectangle {
        color: control.enabled
               ? (control.hovered ? Theme.accentHover : Theme.accent)
               : "transparent"
        border.width: control.enabled ? 0 : 1
        border.color: Theme.border
        radius: Theme.radiusNormal

        Behavior on color {
            ColorAnimation { duration: Theme.durFast }
        }
    }
    contentItem: Text {
        text: control.text
        color: control.enabled ? Theme.accentForeground : Theme.textMuted
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        font: control.font
    }
}
