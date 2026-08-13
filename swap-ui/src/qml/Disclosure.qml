import QtQuick
import QtQuick.Layouts
import SwapTheme

// Collapsible "advanced" section: a quiet clickable header with a rotating
// caret, and content that is present in the layout only while open.
//
// This is how the app does progressive disclosure — the alternative it replaces
// is a card that vanishes via `visible:`, which loses the affordance entirely
// and leaves the user unable to tell that anything was ever there.
ColumnLayout {
    id: disclosure

    property string label
    property bool expanded: false
    // Optional trailing count, e.g. "2 need attention". Rendered in `badgeTone`
    // so a collapsed section can still admit it is hiding something that
    // matters — otherwise progressive disclosure quietly becomes concealment.
    property string badge: ""
    property color badgeTone: Theme.toneAttention
    default property alias bodyData: body.data

    Layout.fillWidth: true
    spacing: Theme.spacingSmall

    // Plain Item, not a RowLayout: the MouseArea has to cover the whole header
    // strip, and an anchor-filled child of a layout is undefined behaviour.
    Item {
        Layout.fillWidth: true
        implicitHeight: headerRow.implicitHeight + Theme.spacingSmall * 2

        RowLayout {
            id: headerRow
            anchors.fill: parent
            spacing: Theme.spacingSmall

            Text {
                text: "▸"
                color: header.containsMouse ? Theme.textSecondary : Theme.textMuted
                font.pixelSize: Theme.fontCaption
                rotation: disclosure.expanded ? 90 : 0
                Behavior on rotation {
                    NumberAnimation { duration: Theme.durFast; easing.type: Easing.OutQuad }
                }
            }
            Text {
                text: disclosure.label
                color: header.containsMouse ? Theme.textSecondary : Theme.textMuted
                font.pixelSize: Theme.fontCaption
                font.bold: true
                font.letterSpacing: 1
            }
            StatusChip {
                visible: disclosure.badge !== ""
                text: disclosure.badge
                tone: disclosure.badgeTone
                status: "attention"
                Layout.alignment: Qt.AlignVCenter
            }
            Item { Layout.fillWidth: true }
        }

        MouseArea {
            id: header
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: disclosure.expanded = !disclosure.expanded

            Accessible.role: Accessible.Button
            Accessible.name: disclosure.label
            Accessible.onPressAction: disclosure.expanded = !disclosure.expanded
        }
    }

    ColumnLayout {
        id: body
        Layout.fillWidth: true
        // Height is not merely hidden but removed, so a collapsed section
        // contributes nothing to the scroll extent.
        visible: disclosure.expanded
        spacing: Theme.spacingNormal
    }
}
