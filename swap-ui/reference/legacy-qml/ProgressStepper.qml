import QtQuick
import QtQuick.Layouts

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
                    width: 28
                    height: 28
                    radius: 14
                    color: {
                        if (stepper.completedSteps.indexOf(modelData.name) >= 0)
                            return Theme.success
                        if (stepper.currentStep === modelData.name)
                            return Theme.accent
                        return Theme.surfaceLight
                    }

                    Text {
                        anchors.centerIn: parent
                        text: stepper.completedSteps.indexOf(modelData.name) >= 0 ? "\u2713" : (index + 1)
                        color: {
                            if (stepper.completedSteps.indexOf(modelData.name) >= 0)
                                return Theme.background
                            if (stepper.currentStep === modelData.name)
                                return "#ffffff"
                            return Theme.textMuted
                        }
                        font.pixelSize: 13
                        font.bold: true
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
                    font.pixelSize: Theme.fontNormal
                    Layout.fillWidth: true
                }
            }

            // Vertical connector line below the circle
            Item {
                visible: index < stepper.steps.length - 1
                Layout.preferredWidth: 28
                Layout.preferredHeight: 16

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
