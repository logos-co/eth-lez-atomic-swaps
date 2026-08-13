import QtQuick
import QtQuick.Layouts
import SwapTheme

// Page title + one-line explanation, with an optional trailing slot for a
// StatusChip.
//
// Titles use fontDisplay rather than fontTitle. Previously every page title
// ("Configuration", "Buy LEZ", "Manual Refund") shared fontTitle with the card
// headings directly beneath them, so nothing on screen announced which page you
// were on.
ColumnLayout {
    id: header

    property string title
    property string subtitle: ""
    // Items placed to the right of the title, e.g. a Live/Offline chip.
    default property alias trailingData: trailing.data

    Layout.fillWidth: true
    spacing: Theme.spacingSmall

    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacingNormal

        Text {
            text: header.title
            color: Theme.textPrimary
            font.pixelSize: Theme.fontDisplay
            font.bold: true
            // Pinned explicitly: the host app's ambient text defaults leak in
            // and justify full-width labels otherwise (0.4.1 live feedback).
            horizontalAlignment: Text.AlignLeft
        }

        RowLayout {
            id: trailing
            Layout.alignment: Qt.AlignVCenter
            spacing: Theme.spacingSmall
        }

        Item { Layout.fillWidth: true }
    }

    Text {
        visible: header.subtitle !== ""
        text: header.subtitle
        color: Theme.textSecondary
        font.pixelSize: Theme.fontNormal
        horizontalAlignment: Text.AlignLeft
        wrapMode: Text.WordWrap
        Layout.fillWidth: true
    }
}
