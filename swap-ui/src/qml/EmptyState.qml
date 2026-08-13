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
    // Call-to-action slot, stacked beneath the subtitle.
    //
    // Children must set `Layout.alignment: Qt.AlignHCenter` themselves. QML
    // layouts have no default child alignment, and there is no arrangement of
    // this slot that centres children automatically: sized to its content it
    // starves a wrapping Text of any width to wrap against, and filling the
    // width leaves fixed-size children on the left. Filling is the lesser
    // evil, because a missing alignment is visible immediately whereas
    // unwrapped text silently overflows the empty state.
    default property alias actionData: actions.data

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
                    from: 16; to: 48; duration: Theme.durPing
                    easing.type: Easing.OutQuad
                }
                NumberAnimation {
                    target: beaconRing; property: "height"
                    from: 16; to: 48; duration: Theme.durPing
                    easing.type: Easing.OutQuad
                }
                NumberAnimation {
                    target: beaconRing; property: "opacity"
                    from: 0.9; to: 0.0; duration: Theme.durPing
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
        // Callers quote backend diagnostics here (the offer board relays
        // swapBackend.messagingHint), so this is not guaranteed to be
        // author-written copy. Plain text only.
        textFormat: Text.PlainText
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
        textFormat: Text.PlainText
        text: empty.subtitle
        color: Theme.textSecondary
        font.pixelSize: Theme.fontNormal
    }

    // Trailing slot for a call-to-action and any footnote beneath it. Exists
    // so the offer board can drop its line-for-line copy of this component;
    // the only things its inline version had that this one lacked were a
    // Button and a "scanning every Ns" footer.
    ColumnLayout {
        id: actions
        // The slot spans the full width and children centre themselves, so a
        // caller's wrapping Text has a real width to wrap against.
        //
        // maximumWidth is not redundant with fillWidth. A NESTED layout caps
        // its own maximumWidth at its implicit content width, so fillWidth
        // alone leaves this column exactly as wide as its widest child and
        // pinned to the left edge — the CTA then sits visibly off-centre under
        // a centred headline. (Same family as the offer table's
        // minimumWidth-defaults-to-implicit-width footgun; see OfferBoard's
        // column model.) Releasing the cap is what makes fillWidth mean what
        // it says here.
        Layout.fillWidth: true
        Layout.maximumWidth: Number.POSITIVE_INFINITY
        Layout.topMargin: Theme.spacingSmall
        // Nothing in the slot means no slot: otherwise the topMargin and the
        // parent's spacing still reserve ~24px under every action-less empty
        // state.
        visible: actions.children.length > 0
        spacing: Theme.spacingSmall
    }
}
