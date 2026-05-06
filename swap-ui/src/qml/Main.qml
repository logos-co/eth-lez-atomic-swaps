import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Assessment scaffold for the LEZ <> ETH atomic swap UI.
//
// The rich legacy QML lives in git history under logos-module/qml/* and should
// be ported into this directory tab-by-tab as swap_ui.rep grows to cover the
// property/slot surface they rely on. Keep this file as the entry point.
// To recover a legacy file:
//   git show <pre-deletion-rev>:logos-module/qml/MakerView.qml > src/qml/MakerView.qml
Item {
    id: root

    // Typed Qt Remote Objects replica — auto-synced properties + callable slots.
    readonly property var backend: logos.module("swap_ui")
    property bool backendReady: false
    readonly property bool ready: backend !== null && backendReady

    // Properties from swap_ui.rep
    readonly property string status: backend ? backend.status : ""
    readonly property string swapRole: backend ? backend.swapRole : ""
    readonly property bool running: backend ? backend.running : false
    readonly property string lastResultJson: backend ? backend.lastResultJson : ""

    Timer {
        interval: 250
        running: true
        repeat: true
        onTriggered: root.backendReady = root.backend !== null
            && logos.isViewModuleReady("swap_ui")
    }

    function setRole(role) {
        if (!root.ready) return;
        logos.watch(backend.setRole(role),
            function(_v) { /* role updated */ },
            function(err) { console.log("setRole error:", err) }
        );
    }

    function fetchBalances() {
        if (!root.ready) return;
        // TODO: replace with real config from a settings panel / env loader.
        var configJson = "{}";
        logos.watch(backend.fetchBalances(configJson),
            function(_v) { /* lastResultJson updated via property sync */ },
            function(err) { console.log("fetchBalances error:", err) }
        );
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 24
        spacing: 16

        Text {
            text: "LEZ ↔ ETH Atomic Swap"
            font.pixelSize: 22
            color: "#ffffff"
        }

        Text {
            text: root.ready ? "Connected" : "Connecting to backend..."
            color: root.ready ? "#56d364" : "#f0883e"
            font.pixelSize: 12
        }

        // Role selector ------------------------------------------------------
        RowLayout {
            spacing: 8

            Text {
                text: "Role:"
                color: "#ffffff"
                Layout.alignment: Qt.AlignVCenter
            }

            Button {
                text: "Maker"
                enabled: root.ready && !root.running
                highlighted: root.swapRole === "maker"
                onClicked: root.setRole("maker")
            }

            Button {
                text: "Taker"
                enabled: root.ready && !root.running
                highlighted: root.swapRole === "taker"
                onClicked: root.setRole("taker")
            }
        }

        // Actions ------------------------------------------------------------
        RowLayout {
            spacing: 8

            Button {
                text: root.running ? "Working..." : "Fetch Balances"
                enabled: root.ready && !root.running
                onClicked: root.fetchBalances()
            }
        }

        // Status + results ---------------------------------------------------
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 40
            color: "#1a1a1a"
            radius: 6

            Text {
                anchors.verticalCenter: parent.verticalCenter
                anchors.left: parent.left
                anchors.leftMargin: 12
                text: "Status: " + root.status
                color: "#8b949e"
                font.pixelSize: 13
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            color: "#0d1117"
            radius: 6
            border.color: "#30363d"
            border.width: 1

            ScrollView {
                anchors.fill: parent
                anchors.margins: 8
                clip: true

                Text {
                    text: root.lastResultJson.length > 0
                        ? root.lastResultJson
                        : "No results yet. Pick a role and call an action."
                    color: "#c9d1d9"
                    font.family: "monospace"
                    font.pixelSize: 12
                    wrapMode: Text.Wrap
                    width: parent.width
                }
            }
        }
    }
}
