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
    title: {
        var base = "Atomic Swaps — LEZ / ETH"
        if (swapBackend.swapRole === "maker") return base + " [MAKER]"
        if (swapBackend.swapRole === "taker") return base + " [TAKER]"
        return base
    }
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

        // Role badge
        Rectangle {
            visible: swapBackend.swapRole === "maker" || swapBackend.swapRole === "taker"
            Layout.fillWidth: true
            height: 32
            color: swapBackend.swapRole === "maker" ? "#1a2e1a" : "#1a1a2e"

            RowLayout {
                anchors.centerIn: parent
                spacing: 8

                Rectangle {
                    width: 8; height: 8
                    radius: 4
                    color: swapBackend.swapRole === "maker" ? Theme.success : "#6c8cff"
                }
                Text {
                    text: swapBackend.swapRole === "maker" ? "MAKER INSTANCE" : "TAKER INSTANCE"
                    color: swapBackend.swapRole === "maker" ? Theme.success : "#6c8cff"
                    font.pixelSize: 12
                    font.bold: true
                    font.letterSpacing: 1.5
                }
                Rectangle {
                    width: 8; height: 8
                    radius: 4
                    color: swapBackend.swapRole === "maker" ? Theme.success : "#6c8cff"
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
                    text: "Atomic Swaps PoC"
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontSmall
                }
            }
        }
    }
}
