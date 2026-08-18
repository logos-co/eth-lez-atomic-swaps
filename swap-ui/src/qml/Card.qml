import QtQuick
import QtQuick.Layouts
import SwapTheme

// The app's panel surface. This idiom was copy-pasted as a bare Rectangle in
// roughly twenty places; the canonical form it replaces is MakerView's offer
// summary card.
//
// Content goes in the default property and is laid out in a ColumnLayout with
// standard padding, so callers no longer hand-compute
// `implicitHeight: someCol.implicitHeight + Theme.spacingNormal * 2`.
//
//   Card {
//       tone: "done"
//       SectionHeader { label: "Your rate" }
//       Text { text: "..." }
//   }
Rectangle {
    id: card

    // Border treatment: "neutral" | "done" | "active" | "problem". "done" and
    // "active" replace the hand-rolled
    // `border.color: x ? Theme.success : Theme.border` ternaries in SetupView
    // and MakerView.
    property string tone: "neutral"

    // Set false for a flush card whose content manages its own insets (the
    // offer table, which needs its rows to reach the card edge).
    property bool padded: true
    property alias spacing: content.spacing
    default property alias contentData: content.data

    Layout.fillWidth: true
    implicitHeight: content.implicitHeight
                    + (padded ? Theme.spacingNormal * 2 : 0)

    color: Theme.surfaceRaised
    radius: Theme.radiusNormal
    border.width: 1
    border.color: {
        switch (card.tone) {
        case "done":    return Theme.toneLive
        case "active":  return Theme.toneWorking
        case "problem": return Theme.toneProblem
        default:        return Theme.border
        }
    }

    Behavior on border.color {
        ColorAnimation { duration: Theme.durNormal }
    }

    ColumnLayout {
        id: content
        anchors {
            fill: parent
            margins: card.padded ? Theme.spacingNormal : 0
        }
        spacing: Theme.spacingSmall
    }
}
