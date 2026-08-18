// Dev-only visual harness. NOT shipped — the module builder copies src/qml
// wholesale into the LGX package, so this deliberately lives outside it.
//
// Main.qml is fully null-guarded (every facade member is
// `root.backend ? root.backend.X : <safe default>`, and even the `logos` bridge
// is behind a `typeof` check), so it renders standalone with no backend at all.
// That gives us the real shell and every real view in their unconfigured state
// — which is exactly the state that was never designed.
//
// Grabs a screenshot and exits, so it can run unattended.
//
//   qml -I swap-ui/src/qml swap-ui/dev/preview.qml
//
// Override the defaults with -- -tab=3 -out=/tmp/x.png -w=1600 -h=1000
import QtQuick
import QtQuick.Window
import "../src/qml"

Window {
    id: win

    function arg(name, fallback) {
        var args = Qt.application.arguments
        for (var i = 0; i < args.length; i++) {
            if (args[i].indexOf("-" + name + "=") === 0)
                return args[i].split("=")[1]
        }
        return fallback
    }

    readonly property int tabIndex: parseInt(arg("tab", "0"))
    readonly property string outPath: arg("out", "/tmp/swap-ui-preview.png")

    width: parseInt(arg("w", "1400"))
    height: parseInt(arg("h", "900"))
    visible: true
    color: "#171717"
    title: "swap-ui preview"

    Main {
        id: app
        anchors.fill: parent
    }

    // Two beats: one for the QML to settle, one after selecting the tab. The
    // shell's own first-run logic can move the tab on us (it jumps to Setup
    // once config validation settles), so select AFTER that has had time to
    // fire rather than racing it.
    Timer {
        interval: 1800
        running: true
        repeat: false
        onTriggered: {
            var bar = findTabBar(app)
            if (bar) bar.currentIndex = win.tabIndex
            grabTimer.start()
        }
    }

    Timer {
        id: grabTimer
        interval: 700
        repeat: false
        onTriggered: win.contentItem.grabToImage(function (result) {
            result.saveToFile(win.outPath)
            Qt.callLater(Qt.quit)
        }, Qt.size(win.width, win.height))
    }

    // The tab bar is private to AtomicSwapView; walk the tree rather than
    // widening the view's API purely for a dev harness.
    function findTabBar(item) {
        if (!item) return null
        if (item.hasOwnProperty("currentIndex") && item.hasOwnProperty("contentWidth")
                && item.toString().indexOf("TabBar") === 0)
            return item
        for (var i = 0; i < item.children.length; i++) {
            var found = findTabBar(item.children[i])
            if (found) return found
        }
        return null
    }
}
