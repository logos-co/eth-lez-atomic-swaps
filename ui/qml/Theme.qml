pragma Singleton
import QtQuick

QtObject {
    readonly property color background: "#1a1a2e"
    readonly property color surface: "#16213e"
    readonly property color surfaceLight: "#1f2f50"
    readonly property color accent: "#e94560"
    readonly property color accentHover: "#ff6b81"
    readonly property color textPrimary: "#eaeaea"
    readonly property color textSecondary: "#8892a4"
    readonly property color textMuted: "#5a6478"
    readonly property color success: "#4ecca3"
    readonly property color warning: "#f9a826"
    readonly property color error: "#e94560"
    readonly property color border: "#2a3a5e"
    readonly property color inputBackground: "#0f1729"

    readonly property int fontSmall: 13
    readonly property int fontNormal: 15
    readonly property int fontLarge: 18
    readonly property int fontTitle: 24

    readonly property int radiusSmall: 6
    readonly property int radiusNormal: 8
    readonly property int radiusLarge: 12

    readonly property int inputHeight: 40

    readonly property int spacingSmall: 8
    readonly property int spacingNormal: 16
    readonly property int spacingLarge: 24
    readonly property int spacingXLarge: 32
}
