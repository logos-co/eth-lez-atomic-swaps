import QtQuick
import QtQuick.Layouts
import SwapTheme

ColumnLayout {
    id: stepper
    spacing: 0

    required property var steps       // [{name: "StepName", label: "Display Label"}, ...]
    required property string currentStep
    required property var completedSteps  // list of step names that are done

    Repeater {
        model: stepper.steps

        ColumnLayout {
            required property var modelData
            required property int index

            Layout.fillWidth: true
            spacing: 0

            RowLayout {
                Layout.fillWidth: true
                spacing: Theme.spacingSmall

                // Step indicator circle
                Rectangle {
                    id: circle
                    width: 24
                    height: 24
                    radius: 12
                    color: {
                        if (stepper.completedSteps.indexOf(modelData.name) >= 0)
                            return Theme.success
                        if (stepper.currentStep === modelData.name)
                            return Theme.accent
                        return Theme.surfaceLight
                    }

                    // Pulse the active step (live-dot idiom)
                    SequentialAnimation on opacity {
                        running: stepper.currentStep === modelData.name && stepper.visible
                        loops: Animation.Infinite
                        NumberAnimation { from: 1.0; to: 0.55; duration: 900 }
                        NumberAnimation { from: 0.55; to: 1.0; duration: 900 }
                        onRunningChanged: if (!running) circle.opacity = 1.0
                    }

                    Text {
                        anchors.centerIn: parent
                        text: stepper.completedSteps.indexOf(modelData.name) >= 0 ? "\u2713" : (index + 1)
                        color: {
                            if (stepper.completedSteps.indexOf(modelData.name) >= 0)
                                return Theme.background
                            if (stepper.currentStep === modelData.name)
                                return Theme.accentForeground
                            return Theme.textMuted
                        }
                        font.pixelSize: Theme.fontDetail
                        font.bold: true
                        font.family: Theme.monoFont
                    }
                }

                // Step label
                Text {
                    text: modelData.label
                    color: {
                        if (stepper.completedSteps.indexOf(modelData.name) >= 0)
                            return Theme.success
                        if (stepper.currentStep === modelData.name)
                            return Theme.textPrimary
                        return Theme.textMuted
                    }
                    font.pixelSize: Theme.fontSmall
                    font.bold: stepper.currentStep === modelData.name
                    Layout.fillWidth: true
                }
            }

            // Vertical connector line below the circle
            Item {
                visible: index < stepper.steps.length - 1
                Layout.preferredWidth: 24
                Layout.preferredHeight: 14

                Rectangle {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 2
                    height: parent.height
                    color: stepper.completedSteps.indexOf(modelData.name) >= 0
                           ? Theme.success : Theme.border
                }
            }
        }
    }
}
