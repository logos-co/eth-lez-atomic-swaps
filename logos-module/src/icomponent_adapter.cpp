#ifdef LOGOS_APP_PLUGIN

#include <IComponent.h>
#include <QObject>
#include <QQuickWidget>
#include <QQmlContext>
#include <QQmlEngine>
#include <QPalette>
#include <QApplication>
#include <QDebug>

#include "lez_atomic_swap_module.h"

// ---------------------------------------------------------------------------
// IComponent implementation for logos-app
// ---------------------------------------------------------------------------
class LezAtomicSwapComponent : public QObject, public IComponent
{
    Q_OBJECT
    Q_INTERFACES(IComponent)
    Q_PLUGIN_METADATA(IID IComponent_iid FILE "../metadata_app.json")

public:
    explicit LezAtomicSwapComponent(QObject *parent = nullptr)
        : QObject(parent)
    {}

    ~LezAtomicSwapComponent() override
    {
        destroyWidget(m_widget);
    }

    QWidget *createWidget(LogosAPI *logosAPI) override
    {
        Q_UNUSED(logosAPI);
        qDebug() << "[AtomicSwap] createWidget called";

        if (m_widget)
            return m_widget;

        // Dark palette matching our Theme.qml
        QApplication *app = qobject_cast<QApplication *>(QApplication::instance());
        if (app) {
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
            app->setPalette(dark);
        }

        // Don't call QQuickStyle::setStyle() — logos-app already loaded
        // QML controls, so calling it here is too late and triggers a warning.

        // Initialize our module
        m_module = new LezAtomicSwapModule(this);
        m_module->initLogos();

        // Create the QQuickWidget that hosts our QML
        m_widget = new QQuickWidget();
        m_widget->setResizeMode(QQuickWidget::SizeRootObjectToView);

        QQmlEngine *engine = m_widget->engine();

        // Register our backend on the engine
        m_module->registerQmlTypes(engine);

        // Load the root QML — use AtomicSwapView directly (not Main.qml
        // which uses ApplicationWindow, incompatible with QQuickWidget embedding).
        m_widget->setSource(QUrl("qrc:/qt/qml/LezAtomicSwap/AtomicSwapView.qml"));

        return m_widget;
    }

    void destroyWidget(QWidget *widget) override
    {
        if (widget) {
            delete widget;
            if (widget == m_widget)
                m_widget = nullptr;
        }
    }

private:
    QQuickWidget *m_widget = nullptr;
    LezAtomicSwapModule *m_module = nullptr;
};

#include "icomponent_adapter.moc"

#endif // LOGOS_APP_PLUGIN
