import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme

ColumnLayout {
    id: stepper
    spacing: 0

    // steps: [{name, label, hint?}, ...]. An optional `hint` renders as one
    // calm expectation line under the ACTIVE step (e.g. typical duration and
    // the auto-refund guarantee) so a normal multi-minute testnet wait reads
    // as "progressing", not "stuck".
    required property var steps
    required property string currentStep
    required property var completedSteps  // list of step names that are done

    // Per-phase liveness (same idea as SetupView's funding ticker, #113): a
    // spinner plus an elapsed-seconds counter for the current step. Waits on
    // steps 4/5 are normally minutes (LEZ blocks can be a minute+ apart) with
    // no intervening event — without a moving number the app looks frozen.
    // Public so an owning view can bind a live countdown (auto-refund) to the
    // same 1-second tick without spinning up a second Timer.
    property int activeElapsedSeconds: 0
    property bool showActiveDetail: true

    onCurrentStepChanged: activeElapsedSeconds = 0

    function fmtElapsed(sec) {
        if (sec < 60) return sec + "s"
        var m = Math.floor(sec / 60)
        return m + "m " + (sec % 60) + "s"
    }

    // Ticks only while the stepper is on screen (the progress card is shown
    // solely during an active swap) and parked on a real step. Reset to 0 on
    // every step change, so it always reads "time spent in THIS phase".
    Timer {
        running: stepper.visible && stepper.currentStep !== ""
        interval: 1000
        repeat: true
        onTriggered: stepper.activeElapsedSeconds += 1
    }

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

            // Active-step liveness detail: spinner + per-phase elapsed, then
            // one expectation line. Indented to sit under the label column.
            ColumnLayout {
                visible: stepper.showActiveDetail
                         && stepper.currentStep === modelData.name
                Layout.fillWidth: true
                Layout.leftMargin: 24 + Theme.spacingSmall
                Layout.topMargin: 2
                spacing: 2

                RowLayout {
                    spacing: Theme.spacingSmall

                    BusyIndicator {
                        running: stepper.currentStep === modelData.name
                                 && stepper.visible
                        implicitWidth: 14
                        implicitHeight: 14
                    }
                    Text {
                        text: stepper.fmtElapsed(stepper.activeElapsedSeconds)
                        color: Theme.textSecondary
                        font.pixelSize: Theme.fontCaption
                        font.family: Theme.monoFont
                    }
                }

                Text {
                    visible: text !== ""
                    text: modelData.hint !== undefined ? modelData.hint : ""
                    color: Theme.textMuted
                    font.pixelSize: Theme.fontCaption
                    horizontalAlignment: Text.AlignLeft
                    wrapMode: Text.WordWrap
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
