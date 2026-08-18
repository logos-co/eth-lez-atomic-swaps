import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme
import SwapFormat

// One row of the trust surface: a label, a truncated monospace value, and quiet
// copy / explorer-link controls.
//
// Replaces three competing idioms — TrustRow (label stacked over value over two
// full-size buttons), OfferBoard's private DetailField (same layout, but with
// NO copy and NO link, on the one screen where you verify a maker's address
// before locking ETH), and nine hand-rolled substring truncations using six
// different head/tail shapes and two different ellipsis characters.
//
// Truncation always goes through Format.shortHex, so the same value is clipped
// identically everywhere. Click the value to toggle the full string — a
// truncated value on screen therefore always means "there is more", and the
// full text stays reachable without a copy round trip.
//
// The link control copies the URL rather than opening it: Basecamp blocks
// module-owned external navigation (issue #84), and the external-open call
// silently no-ops there. Copy is the honest affordance, not a workaround.
// (tests/check-feedback-evidence.mjs enforces that no QML names that call —
// hence the circumlocution here.)
RowLayout {
    id: row

    property string label
    property string value
    // A resolved explorer URL from SwapLinks. Empty means no link control.
    property string link: ""
    // Shared across sibling rows so labels and values line up in a column.
    property int labelWidth: 128
    // Hold the link slot open even on a row that has no link. Set this on a
    // STACK of rows where only some are linkable (the offer board's trust
    // surface: LEZ values resolve to an explorer, ETH values don't) — otherwise
    // the copy buttons land in two different columns and the eye reads the
    // ragged edge as the rows being different kinds of thing. Off by default,
    // so a lone row doesn't reserve 26px of nothing.
    property bool reserveLinkSlot: false
    property int headChars: 8
    property int tailChars: 6
    property color valueColor: Theme.textSecondary

    readonly property bool hasValue: value !== ""
    readonly property bool hasLink: hasValue && link !== ""
    readonly property bool canExpand: Format.isTruncated(value, headChars, tailChars)

    property bool expanded: false
    property string copiedKind: ""

    Layout.fillWidth: true
    spacing: Theme.spacingNormal

    // Pure-QML clipboard: an invisible TextEdit whose selection is copied.
    TextEdit {
        id: clipboard
        visible: false
    }

    Timer {
        id: copiedReset
        interval: 1600
        onTriggered: row.copiedKind = ""
    }

    function copyText(text, kind) {
        clipboard.text = text
        clipboard.selectAll()
        clipboard.copy()
        clipboard.text = ""
        row.copiedKind = kind
        copiedReset.restart()
    }

    Text {
        Layout.preferredWidth: row.labelWidth
        Layout.alignment: Qt.AlignTop
        // Labels are authored in QML, not carried on the wire — but AutoText
        // decides per-string, so leaving it unset here would mean the one
        // component that renders untrusted values has two different rules
        // inside it. One rule: this component never renders rich text.
        textFormat: Text.PlainText
        text: row.label
        color: Theme.textMuted
        font.pixelSize: Theme.fontMicro
        font.letterSpacing: 0.5
        horizontalAlignment: Text.AlignLeft
        wrapMode: Text.WordWrap
    }

    Text {
        id: valueText
        Layout.fillWidth: true
        Layout.alignment: Qt.AlignTop
        // Every value on this surface is offer- or receipt-derived, i.e.
        // attacker-controlled. Text defaults to AutoText, which sniffs the
        // string and renders anything HTML-ish as rich text — so a maker
        // could style, hide or fake characters in an address the user is
        // reading precisely to decide whether to lock ETH. Never rich text.
        textFormat: Text.PlainText
        text: row.hasValue
              ? (row.expanded ? row.value
                              : Format.shortHex(row.value, row.headChars, row.tailChars))
              : "—"
        color: row.valueColor
        font.pixelSize: Theme.fontCaption
        font.family: Theme.monoFont
        // Wrap (not WordWrap): an expanded hex string is one unbreakable token
        // and must break mid-token rather than overflow.
        wrapMode: Text.Wrap

        MouseArea {
            anchors.fill: parent
            enabled: row.canExpand
            hoverEnabled: row.canExpand
            cursorShape: row.canExpand ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: row.expanded = !row.expanded

            ToolTip.visible: containsMouse
            ToolTip.delay: 400
            ToolTip.text: row.expanded ? "Hide the full value" : "Show the full value"
        }
    }

    // --- Quiet glyph controls -------------------------------------------
    component GlyphButton: Button {
        id: glyph
        property string tip
        Layout.preferredWidth: 26
        Layout.preferredHeight: 26
        Layout.alignment: Qt.AlignTop
        flat: true

        Accessible.role: Accessible.Button
        Accessible.name: glyph.tip
        Accessible.onPressAction: glyph.clicked()

        background: Rectangle {
            color: glyph.hovered ? Theme.surfaceLight : "transparent"
            radius: Theme.radiusSmall
        }
        contentItem: Text {
            text: glyph.text
            color: glyph.hovered ? Theme.textPrimary : Theme.textMuted
            font.pixelSize: Theme.fontDetail
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }

        ToolTip.visible: hovered
        ToolTip.delay: 400
        ToolTip.text: glyph.tip
    }

    GlyphButton {
        visible: row.hasValue
        text: row.copiedKind === "value" ? "✓" : "⧉"
        tip: row.copiedKind === "value" ? "Copied" : "Copy " + row.label
        onClicked: row.copyText(row.value, "value")
    }

    GlyphButton {
        // Kept in the layout but inert when reserving: `visible: false` would
        // remove it and collapse the column again.
        visible: row.hasLink || row.reserveLinkSlot
        enabled: row.hasLink
        opacity: row.hasLink ? 1 : 0
        text: row.copiedKind === "link" ? "✓" : "↗"
        tip: row.copiedKind === "link"
             ? "Explorer link copied"
             : "Copy a block-explorer link (paste it in your browser)"
        onClicked: row.copyText(row.link, "link")
    }
}
