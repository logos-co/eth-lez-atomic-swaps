pragma Singleton
import QtQuick

QtObject {
    // Logos Design System colors
    readonly property color background: "#171717"
    readonly property color surface: "#1E1E1E"
    readonly property color surfaceLight: "#2A2A2A"
    readonly property color accent: "#FF8800"
    readonly property color accentHover: "#FF9922"
    readonly property color textPrimary: "#F0F0F0"
    readonly property color textSecondary: "#999999"
    readonly property color textMuted: "#666666"
    readonly property color success: "#4ecca3"
    readonly property color warning: "#f9a826"
    readonly property color error: "#e94560"
    readonly property color border: "#333333"
    readonly property color inputBackground: "#1A1A1A"
    readonly property color accentForeground: "#ffffff"

    // Elevation. `surface` sits only 3% above `background`, which reads as a
    // hairline outline rather than a panel. `surfaceRaised` is the card face;
    // `surfaceSunken` is for input wells, so a field reads as a hole in the
    // card rather than another card on top of it.
    readonly property color surfaceRaised: "#242424"
    readonly property color surfaceSunken: "#141414"

    // Semantic status tones. Without these every view independently decided
    // that "waiting" means warning, so ordinary preconditions ("a swap is
    // already running") ended up styled as hazards. Map state -> tone here
    // once; see StatusDot.qml for the matching dot vocabulary.
    readonly property color toneLive: success        // healthy + running
    readonly property color toneWorking: accent      // busy, we are doing it
    readonly property color toneWaiting: textSecondary  // normal, just not yet
    readonly property color toneAttention: warning   // user should look
    readonly property color toneProblem: error       // it failed

    // Status-vocabulary lookups. These live here rather than on StatusDot so
    // that StatusDot and StatusChip can each derive from `status`
    // independently — having one read the other's `tone` is a binding loop.
    function toneFor(status) {
        switch (status) {
        case "live":      return toneLive
        case "working":   return toneWorking
        case "attention": return toneAttention
        case "problem":   return toneProblem
        default:          return toneWaiting
        }
    }

    // Only the two "something is happening right now" states pulse. A steady
    // dot means the state is stable, whether that is good or bad.
    function pulsesFor(status) {
        return status === "live" || status === "working"
    }

    // Monospace is the app's "data/trust" voice: amounts, addresses,
    // hashes, countdowns. Everything else stays in the default UI face.
    readonly property string monoFont: "Menlo, Courier New"

    readonly property int fontMicro: 10    // trust-surface field labels
    readonly property int fontCaption: 11  // meta text, hints, chip labels
    readonly property int fontDetail: 12   // secondary rows
    readonly property int fontSmall: 13
    readonly property int fontNormal: 15
    readonly property int fontLarge: 18
    readonly property int fontTitle: 24
    // Page titles. Previously every page title shared fontTitle with the card
    // headings beneath it, so nothing announced which screen you were on.
    readonly property int fontDisplay: 32

    readonly property int radiusSmall: 6
    readonly property int radiusNormal: 8
    readonly property int radiusLarge: 12

    readonly property int inputHeight: 40

    readonly property int spacingSmall: 8
    readonly property int spacingNormal: 16
    readonly property int spacingLarge: 24
    readonly property int spacingXLarge: 32

    // Reading-column widths. Views fill the whole window otherwise, which on a
    // wide display stretches a single-line field past 1700px. `contentMaxWidth`
    // is for forms and prose; `boardMaxWidth` is for the offer table, which
    // genuinely wants the extra columns.
    readonly property int contentMaxWidth: 880
    readonly property int boardMaxWidth: 1400

    // Motion. The pulse/ping durations were literals inside StatusChip and
    // EmptyState and had already drifted apart (900 vs 1200 vs 1800ms).
    readonly property int durFast: 150
    readonly property int durNormal: 300
    readonly property int durPulse: 900     // "still alive" dot pulse
    readonly property int durPing: 1800     // radar-ping beacon ring
}
