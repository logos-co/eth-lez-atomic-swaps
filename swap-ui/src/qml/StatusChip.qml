import QtQuick
import QtQuick.Layouts
import SwapTheme

// Bordered status pill with a small dot — the offer board's chip idiom
// (config-readiness chip, "+ Make an offer" CTA). Set `clickable: true`
// to get hover feedback and the clicked() signal.
Rectangle {
    id: chip

    property string text
    property color tone: Theme.textSecondary
    property bool showDot: true
    property bool bold: false
    property bool clickable: false
    // Pulses the dot (the board's live-dot treatment).
    property bool pulsing: false
    signal clicked()

    implicitWidth: chipRow.implicitWidth + Theme.spacingNormal
    implicitHeight: 24
    radius: height / 2
    color: chip.clickable && chipMouse.containsMouse
           ? Theme.surfaceLight : "transparent"
    border.color: chip.tone
    border.width: 1

    RowLayout {
        id: chipRow
        anchors.centerIn: parent
        spacing: 6

        Rectangle {
            id: chipDot
            visible: chip.showDot
            width: 6; height: 6; radius: 3
            color: chip.tone

            SequentialAnimation on opacity {
                running: chip.pulsing && chip.visible
                loops: Animation.Infinite
                NumberAnimation { from: 1.0; to: 0.35; duration: 900 }
                NumberAnimation { from: 0.35; to: 1.0; duration: 900 }
            }
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
