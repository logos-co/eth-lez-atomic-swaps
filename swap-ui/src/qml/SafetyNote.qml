import QtQuick
import QtQuick.Layouts
import SwapTheme

// The reassurance motif: "your funds can't be lost; worst case they come back."
//
// This app's whole premise is that a stalled swap is survivable, and the copy
// that says so was written well — but it lived in exactly five places, four of
// which were the taker's happy path. It was missing from the Refund screen (the
// one screen a frightened user actually lands on), from the entire maker side
// (whose LEZ sits in escrow with nothing saying it returns), from the
// confirm-purchase click, and from every error surface.
//
// Deliberately styled calm, not amber. Amber in this app means "look at this";
// a safety net is the opposite of an alarm.
//
// Honesty rule: only pass a `deadline` the chain can actually back. If there is
// no real timelock to quote, say the general truth and quote nothing —
// inventing a countdown here would undermine the one thing this component
// exists to establish.
RowLayout {
    id: note

    property string text
    // Optional "by 05:09:50" style detail, appended muted after the sentence.
    property string deadline: ""

    Layout.fillWidth: true
    spacing: Theme.spacingSmall

    Rectangle {
        Layout.alignment: Qt.AlignTop
        Layout.topMargin: 2
        Layout.preferredWidth: 3
        Layout.preferredHeight: body.implicitHeight
        radius: 1.5
        color: Theme.toneLive
        opacity: 0.7
    }

    Text {
        id: body
        Layout.fillWidth: true
        text: note.deadline !== ""
              ? note.text + " " + note.deadline
              : note.text
        color: Theme.textSecondary
        font.pixelSize: Theme.fontSmall
        horizontalAlignment: Text.AlignLeft
        wrapMode: Text.WordWrap
    }
}
