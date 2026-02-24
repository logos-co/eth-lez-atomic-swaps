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
