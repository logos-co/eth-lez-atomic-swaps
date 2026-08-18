import QtQuick
import QtQuick.Layouts
import SwapTheme

// Bordered status pill. Set `status` to one of the five StatusDot states and
// the dot, colour and pulse all follow from it — that is the whole point, and
// it is why `tone`/`pulsing` are overrides rather than the primary API.
//
// Set `clickable: true` for hover feedback and the clicked() signal. Set
// `showDot: false` to use it as a plain count badge.
Rectangle {
    id: chip

    property string text
    // "live" | "working" | "waiting" | "attention" | "problem"
    property string status: "waiting"
    // Explicit overrides; both default to whatever `status` implies. They
    // derive from Theme, not from the child dot, so there is no binding cycle.
    property color tone: Theme.toneFor(chip.status)
    property bool pulsing: Theme.pulsesFor(chip.status)
    property bool showDot: true
    property bool bold: false
    property bool clickable: false
    signal clicked()

    implicitWidth: chipRow.implicitWidth + Theme.spacingNormal
    implicitHeight: 24
    radius: height / 2
    color: chip.clickable && chipMouse.containsMouse
           ? Theme.surfaceLight : "transparent"
    border.color: chip.tone
    border.width: 1

    Behavior on border.color {
        ColorAnimation { duration: Theme.durNormal }
    }

    RowLayout {
        id: chipRow
        anchors.centerIn: parent
        spacing: 6

        StatusDot {
            id: dot
            visible: chip.showDot
            status: chip.status
            size: 6
            // Push the chip's overrides down rather than restyling the dot
            // here, so the dot owns its own pulse animation exclusively.
            tone: chip.tone
            pulsing: chip.pulsing
        }
        Text {
            text: chip.text
            color: chip.tone
            font.pixelSize: Theme.fontCaption
            font.bold: chip.bold
        }
    }

    MouseArea {
        id: chipMouse
        anchors.fill: parent
        enabled: chip.clickable
        hoverEnabled: chip.clickable
        cursorShape: chip.clickable ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: chip.clicked()
    }
}
