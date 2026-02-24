import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root

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

        // Role badge
        Rectangle {
            visible: swapBackend.swapRole === "maker" || swapBackend.swapRole === "taker"
            Layout.fillWidth: true
            height: 32
            color: swapBackend.swapRole === "maker" ? Theme.surface : Theme.surface

            RowLayout {
                anchors.centerIn: parent
                spacing: 8

                Rectangle {
                    width: 8; height: 8
                    radius: 4
                    color: swapBackend.swapRole === "maker" ? Theme.success : Theme.accent
                }
                Text {
                    text: swapBackend.swapRole === "maker" ? "MAKER INSTANCE" : "TAKER INSTANCE"
                    color: swapBackend.swapRole === "maker" ? Theme.success : Theme.accent
                    font.pixelSize: 12
                    font.bold: true
                    font.letterSpacing: 1.5
                }
                Rectangle {
                    width: 8; height: 8
                    radius: 4
                    color: swapBackend.swapRole === "maker" ? Theme.success : Theme.accent
                }
            }
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
                    text: "LEZ Atomic Swap"
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontSmall
                }
            }
        }
    }
}
