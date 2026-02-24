import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    id: root
    width: 800
    height: 700
    minimumWidth: 640
    minimumHeight: 500
    visible: true
    title: "Atomic Swaps — LEZ / ETH"
    color: Theme.background

    background: Rectangle { color: Theme.background }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Tab bar
        TabBar {
            id: tabBar
            Layout.fillWidth: true
            background: Rectangle { color: Theme.surface }

            Repeater {
                model: ["Config", "Maker", "Taker", "Refund"]
                TabButton {
                    text: modelData
                    width: implicitWidth
                    leftPadding: 20
                    rightPadding: 20
                    font.pixelSize: Theme.fontNormal
                    contentItem: Text {
                        text: parent.text
                        font: parent.font
                        color: tabBar.currentIndex === index ? Theme.accent : Theme.textSecondary
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }
                    background: Rectangle {
                        color: tabBar.currentIndex === index ? Theme.surfaceLight : "transparent"
                        Rectangle {
                            anchors.bottom: parent.bottom
                            width: parent.width
                            height: 3
                            color: tabBar.currentIndex === index ? Theme.accent : "transparent"
                        }
                    }
                }
            }
        }

        // Separator line below tab bar
        Rectangle {
            Layout.fillWidth: true
            height: 1
            color: Theme.border
        }

        // Content
        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabBar.currentIndex

            ConfigPanel {}
            MakerView {}
            TakerView {}
            RefundView {}
        }

        // Status bar
        Rectangle {
            Layout.fillWidth: true
            height: 32
            color: Theme.surface

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: Theme.spacingNormal
                anchors.rightMargin: Theme.spacingNormal

                Text {
                    text: swapBackend.running
                          ? "Running: " + swapBackend.currentStep
                          : "Idle"
                    color: swapBackend.running ? Theme.warning : Theme.textMuted
                    font.pixelSize: Theme.fontSmall
                }
                Item { Layout.fillWidth: true }
                Text {
                    text: "Atomic Swaps PoC"
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontSmall
                }
            }
        }
    }
}
