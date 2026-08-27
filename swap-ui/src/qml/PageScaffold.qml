import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme

// The standard page shell: scroll surface, page header, and a centred reading
// column.
//
// Two problems it solves at once.
//
// 1. Width. Every view was a hand-rolled `ScrollView -> Flickable ->
//    ColumnLayout { Layout.fillWidth }`, so content stretched to the whole
//    window — on a wide display that put a single-line password field past
//    1700px. Content here is capped at `maxWidth` and centred.
//
// 2. The #113 footgun. ScrollView adopts the FIRST Flickable in its contentData
//    as its scroll surface; a Timer or Connections declared ahead of it makes
//    ScrollView spin up its own implicit Flickable and collapse the real layout
//    to the origin (the 0.4.2 Setup-tab overlap regression). Because this
//    component owns the Flickable and callers can only append into the content
//    column, that mistake is no longer reachable from a view. Non-visual
//    helpers in the default slot land inside the ColumnLayout, which ignores
//    them for layout purposes — exactly where they belong.
//
//   PageScaffold {
//       title: "Page title"
//       subtitle: "One line of context."
//       Card { ... }
//       Timer { ... }        // safe: not a direct ScrollView child
//   }
ScrollView {
    id: page

    property string title: ""
    property string subtitle: ""
    property int maxWidth: Theme.contentMaxWidth
    // Items rendered to the right of the title (a StatusChip, usually).
    property alias headerTrailingData: header.trailingData
    // NOT named `contentData`: ScrollView derives from Pane, which declares a
    // final `contentData` — shadowing it silently breaks child assignment.
    default property alias pageContent: column.data

    clip: true
    contentWidth: availableWidth
    background: Rectangle { color: Theme.background }

    Flickable {
        id: flick

        contentWidth: page.availableWidth
        contentHeight: column.implicitHeight + Theme.spacingXLarge * 2
        boundsBehavior: Flickable.StopAtBounds

        ColumnLayout {
            id: column

            // Positioned with x/y rather than anchors: inside a Flickable,
            // `parent` is the content item, whose implicit width is not
            // reliable enough to centre against. Falls back to a plain margin
            // once the window is narrower than maxWidth.
            y: Theme.spacingXLarge
            width: Math.min(flick.contentWidth - Theme.spacingXLarge * 2,
                            page.maxWidth)
            x: Math.max(Theme.spacingXLarge, (flick.contentWidth - width) / 2)

            spacing: Theme.spacingLarge

            PageHeader {
                id: header
                visible: page.title !== ""
                title: page.title
                subtitle: page.subtitle
            }
        }
    }
}
