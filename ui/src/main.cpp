#include <QGuiApplication>
#include <QJSValue>
#include <QPalette>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickStyle>
#include <QThreadPool>

#include "swapbackend.h"

int main(int argc, char *argv[])
{
    qputenv("QT_QUICK_CONTROLS_STYLE", "Basic");

    // Alloy/tungstenite WebSocket+TLS handshake needs deep stack.
    // Default QtConcurrent pool threads get ~512K which overflows.
    QThreadPool::globalInstance()->setStackSize(8 * 1024 * 1024);

    QGuiApplication app(argc, argv);
    app.setApplicationName("Atomic Swaps");
    app.setOrganizationName("Logos");

    QQuickStyle::setStyle("Basic");

    // Dark palette so Basic style controls default to dark colors
    QPalette dark;
    dark.setColor(QPalette::Window,     QColor("#1a1a2e"));
    dark.setColor(QPalette::WindowText, QColor("#eaeaea"));
    dark.setColor(QPalette::Base,       QColor("#0f1729"));
    dark.setColor(QPalette::Text,       QColor("#eaeaea"));
    dark.setColor(QPalette::Button,     QColor("#1f2f50"));
    dark.setColor(QPalette::ButtonText, QColor("#eaeaea"));
    dark.setColor(QPalette::Mid,        QColor("#2a3a5e"));
    dark.setColor(QPalette::Dark,       QColor("#0f1729"));
    dark.setColor(QPalette::PlaceholderText, QColor("#5a6478"));
    app.setPalette(dark);

    SwapBackend backend;
    backend.loadEnv();

    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty("swapBackend", &backend);

    // Expose Theme as a context property (workaround for QML singleton
    // not resolving in Qt 6 modules with subdirectory file layout).
    QJSValue theme = engine.newObject();
    // Colors
    theme.setProperty("background",      "#1a1a2e");
    theme.setProperty("surface",         "#16213e");
    theme.setProperty("surfaceLight",    "#1f2f50");
    theme.setProperty("accent",          "#e94560");
    theme.setProperty("accentHover",     "#ff6b81");
    theme.setProperty("textPrimary",     "#eaeaea");
    theme.setProperty("textSecondary",   "#8892a4");
    theme.setProperty("textMuted",       "#5a6478");
    theme.setProperty("success",         "#4ecca3");
    theme.setProperty("warning",         "#f9a826");
    theme.setProperty("error",           "#e94560");
    theme.setProperty("border",          "#2a3a5e");
    theme.setProperty("inputBackground", "#0f1729");
    // Font sizes
    theme.setProperty("fontSmall",  13);
    theme.setProperty("fontNormal", 15);
    theme.setProperty("fontLarge",  18);
    theme.setProperty("fontTitle",  24);
    // Radii
    theme.setProperty("radiusSmall",  6);
    theme.setProperty("radiusNormal", 8);
    theme.setProperty("radiusLarge",  12);
    // Layout
    theme.setProperty("inputHeight",   40);
    theme.setProperty("spacingSmall",  8);
    theme.setProperty("spacingNormal", 16);
    theme.setProperty("spacingLarge",  24);
    theme.setProperty("spacingXLarge", 32);
    engine.rootContext()->setContextProperty("Theme", QVariant::fromValue(theme));

    engine.loadFromModule("AtomicSwaps", "Main");

    if (engine.rootObjects().isEmpty())
        return -1;

    return app.exec();
}
