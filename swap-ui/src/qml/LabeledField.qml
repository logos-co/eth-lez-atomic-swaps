import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme

// Labelled text input with error and hint lines. Promoted verbatim from
// ConfigPanel's file-private `ConfigField` so other views can reuse it.
//
// The caret-preservation logic below is load-bearing — do not "simplify" it.
// It is what stops an async setConfigValue() round trip from stomping the caret
// and reordering keystrokes while the user is still typing (issue #87).
ColumnLayout {
    id: field

    property alias label: labelText.text
    // NOTE: `text` is intentionally a plain property, not `property alias text: input.text`.
    // It represents the backend's value flowing IN. `input.text` is the field's own local
    // buffer. Keeping them separate breaks the echo loop below.
    //
    // THIS IS A CONTROLLED FIELD. `text` is the value; the caller MUST keep it
    // in sync by handling `valueEdited`. Two ways to get this wrong, both of
    // which have already bitten:
    //
    //  1. Reading `field.text` back to validate or submit. It returns what was
    //     pushed IN, not what was typed — so the form ignores every keystroke.
    //     `liveText` (below) is the visible buffer.
    //  2. Using `text:` as a one-time seed and never handling `valueEdited`.
    //     `onActiveFocusChanged` re-applies `text` on every blur, so the box
    //     silently reverts to the seed the moment focus leaves — including the
    //     blur caused by clicking the submit button next to it, which then
    //     submits an empty value.
    //
    // Both are avoided by the same discipline: own a property, bind `text` to
    // it, and write to it from `valueEdited`.
    property string text: ""
    // What the user can actually see in the field, right now.
    readonly property alias liveText: input.text
    property alias echoMode: input.echoMode
    property alias placeholderText: input.placeholderText
    // 0 means unlimited (TextField's own default).
    property alias maximumLength: input.maximumLength
    property bool fieldEnabled: true
    property string errorText: ""
    property string hint: ""
    signal valueEdited(string val)

    Layout.fillWidth: true
    spacing: 4

    // Apply an externally-driven value (from the backend) into the visible field, but only
    // when it's safe to do so: never while the user has focus and is actively typing, and
    // never if it's a no-op. This is what stops an async setConfigValue() round trip from
    // stomping the caret / reordering keystrokes while the user is still mid-edit.
    function applyExternalText(value) {
        if (input.activeFocus) return
        if (input.text === value) return
        var pos = input.cursorPosition
        input.text = value
        input.cursorPosition = Math.min(pos, input.text.length)
    }

    onTextChanged: applyExternalText(field.text)

    Text {
        id: labelText
        color: Theme.textMuted
        font.pixelSize: Theme.fontCaption
        font.letterSpacing: 0.5
    }
    TextField {
        id: input
        enabled: field.fieldEnabled
        Layout.fillWidth: true
        Layout.preferredHeight: Theme.inputHeight
        leftPadding: 12
        rightPadding: 12
        topPadding: 8
        bottomPadding: 8
        color: Theme.textPrimary
        font.pixelSize: Theme.fontSmall
        font.family: Theme.monoFont
        selectByMouse: true
        background: Rectangle {
            // surfaceSunken, so a field reads as a well cut into the card
            // rather than another panel stacked on top of it.
            color: Theme.surfaceSunken
            border.color: field.errorText !== ""
                          ? Theme.error
                          : (input.activeFocus ? Theme.accent : Theme.border)
            border.width: 1
            radius: Theme.radiusSmall
            opacity: input.enabled ? 1.0 : 0.5
        }
        // onTextEdited fires only for genuine user keystrokes (not programmatic assignment),
        // so this no longer re-triggers itself when the backend echo below writes back.
        onTextEdited: field.valueEdited(input.text)
        // When focus is lost, apply whatever the backend's latest value is (it may have been
        // normalized/validated while the user was typing and held back by the guard above).
        onActiveFocusChanged: if (!activeFocus) field.applyExternalText(field.text)
    }
    Text {
        visible: field.errorText !== ""
        text: field.errorText
        color: Theme.error
        font.pixelSize: Theme.fontCaption
        horizontalAlignment: Text.AlignLeft
        wrapMode: Text.Wrap
        Layout.fillWidth: true
    }
    Text {
        visible: field.hint !== "" && field.errorText === ""
        text: field.hint
        color: Theme.textMuted
        font.pixelSize: Theme.fontCaption
        horizontalAlignment: Text.AlignLeft
        wrapMode: Text.Wrap
        Layout.fillWidth: true
    }
}
