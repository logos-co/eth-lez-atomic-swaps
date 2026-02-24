import QtQuick
import QtQuick.Controls

ApplicationWindow {
    id: root
    width: 800
    height: 700
    minimumWidth: 640
    minimumHeight: 500
    visible: true
    title: {
        var base = "LEZ Atomic Swap"
        if (swapBackend.swapRole === "maker") return base + " [MAKER]"
        if (swapBackend.swapRole === "taker") return base + " [TAKER]"
        return base
    }
    color: Theme.background

    background: Rectangle { color: Theme.background }

    AtomicSwapView {
        anchors.fill: parent
    }
}
