#include <QGuiApplication>
#include <QPalette>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickStyle>

#include "lez_atomic_swap_module.h"
#include "swap_backend.h"

int main(int argc, char *argv[])
{
    qputenv("QT_QUICK_CONTROLS_STYLE", "Basic");

    QGuiApplication app(argc, argv);
    app.setApplicationName("LEZ Atomic Swap");
    app.setOrganizationName("Logos");

    QQuickStyle::setStyle("Basic");

    // Dark palette matching Logos Design System
    QPalette dark;
    dark.setColor(QPalette::Window,          QColor("#171717"));
    dark.setColor(QPalette::WindowText,      QColor("#F0F0F0"));
    dark.setColor(QPalette::Base,            QColor("#1A1A1A"));
    dark.setColor(QPalette::Text,            QColor("#F0F0F0"));
    dark.setColor(QPalette::Button,          QColor("#2A2A2A"));
    dark.setColor(QPalette::ButtonText,      QColor("#F0F0F0"));
    dark.setColor(QPalette::Mid,             QColor("#333333"));
    dark.setColor(QPalette::Dark,            QColor("#1A1A1A"));
    dark.setColor(QPalette::PlaceholderText, QColor("#666666"));
    app.setPalette(dark);

    LezAtomicSwapModule module;
    module.initLogos();

    QQmlApplicationEngine engine;
    module.registerQmlTypes(&engine);

    engine.loadFromModule("LezAtomicSwap", "Main");

    if (engine.rootObjects().isEmpty())
        return -1;

    return app.exec();
}
