import QtQuick
import SwapTheme

// The app's single "is it alive?" indicator.
//
// It replaces eight different dot sizes (5/6/8/10/16/24px plus a "●" text glyph
// and a 🛡 emoji), three unrelated liveness motifs (pulsing opacity, expanding
// ring, draining bar) and five pulse animations running at three different
// durations. On the Market tab six of those fired simultaneously to say one
// thing: the network is up.
//
// Five states, and only these five:
//
//   live       green,  pulsing   healthy and running
//   working    orange, pulsing   busy right now — scanning, locking, claiming
//   waiting    grey,   steady    normal, just not yet — waiting on the other side
//   attention  amber,  steady    the user should look — no peers, config gaps
//   problem    red,    steady    it failed
//
// "waiting" is deliberately grey. Ordinary preconditions ("a swap is already
// running", "backend is starting") used to be amber with a ⚠, which trained the
// eye to ignore amber for the cases that genuinely matter.
Rectangle {
    id: dot

    property string status: "waiting"
    property int size: 8

    // Both derive from `status` by default. They are plain (not readonly)
    // properties so a caller can override one without abandoning the
    // vocabulary — assigning either simply replaces its default binding.
    property color tone: Theme.toneFor(dot.status)
    property bool pulsing: Theme.pulsesFor(dot.status)

    implicitWidth: size
    implicitHeight: size
    width: size
    height: size
    radius: size / 2
    color: tone

    Behavior on color {
        ColorAnimation { duration: Theme.durNormal }
    }

    SequentialAnimation on opacity {
        running: dot.pulsing && dot.visible
        loops: Animation.Infinite
        // Reset to fully opaque when the animation stops, so a dot that
        // settles from "working" to "waiting" never freezes mid-fade.
        onStopped: dot.opacity = 1.0
        NumberAnimation { from: 1.0; to: 0.35; duration: Theme.durPulse }
        NumberAnimation { from: 0.35; to: 1.0; duration: Theme.durPulse }
    }
}
