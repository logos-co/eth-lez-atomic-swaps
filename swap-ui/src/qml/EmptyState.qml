import QtQuick
import QtQuick.Layouts
import SwapTheme

// Full-bleed empty/explainer state with the offer board's radar-ping
// beacon: an expanding ring around a core dot, a bold headline, and
// muted supporting copy. Place inside a layout (fills width) or center
// it in an Item.
ColumnLayout {
    id: empty

    property color tone: Theme.accent
    property string title
    property string subtitle
    // Drive the ping only while on screen; defaults to visibility.
    property bool running: empty.visible

    spacing: Theme.spacingNormal

    Item {
        Layout.alignment: Qt.AlignHCenter
        width: 48; height: 48

        Rectangle {
            id: beaconRing
            anchors.centerIn: parent
            width: 16; height: 16; radius: width / 2
            color: "transparent"
            border.width: 2
            border.color: empty.tone

            ParallelAnimation {
                running: empty.running
                loops: Animation.Infinite
                NumberAnimation {
                    target: beaconRing; property: "width"
                    from: 16; to: 48; duration: 1800
                    easing.type: Easing.OutQuad
                }
                NumberAnimation {
                    target: beaconRing; property: "height"
                    from: 16; to: 48; duration: 1800
                    easing.type: Easing.OutQuad
                }
                NumberAnimation {
                    target: beaconRing; property: "opacity"
                    from: 0.9; to: 0.0; duration: 1800
                }
            }
        }
        Rectangle {
            anchors.centerIn: parent
            width: 10; height: 10; radius: 5
            color: empty.tone
        }
    }

    Text {
        visible: empty.title !== ""
        Layout.alignment: Qt.AlignHCenter
        Layout.fillWidth: true
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
        text: empty.title
        color: Theme.textPrimary
        font.pixelSize: Theme.fontTitle
        font.bold: true
    }

    Text {
        visible: empty.subtitle !== ""
        Layout.alignment: Qt.AlignHCenter
        Layout.fillWidth: true
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
        text: empty.subtitle
        color: Theme.textSecondary
        font.pixelSize: Theme.fontNormal
    }
}
