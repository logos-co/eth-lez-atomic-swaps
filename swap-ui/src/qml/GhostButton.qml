import QtQuick
import QtQuick.Controls
import SwapTheme

// Outlined secondary button. `accented: true` is the board's
// accent-outline treatment ("+ Make an offer"); false is a neutral
// ghost for cancel/utility actions.
Button {
    id: control

    property bool accented: true

    font.pixelSize: Theme.fontNormal

    background: Rectangle {
        color: control.enabled && control.hovered
               ? (control.accented
                  ? Theme.surfaceLight
                  : Qt.darker(Theme.surface, 1.1))
               : Theme.surface
        border.color: control.accented && control.enabled
                      ? Theme.accent : Theme.border
        border.width: 1
        radius: Theme.radiusNormal
    }
    contentItem: Text {
        text: control.text
        color: !control.enabled
               ? Theme.textMuted
               : (control.accented ? Theme.accent : Theme.textPrimary)
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        font: control.font
    }
}
