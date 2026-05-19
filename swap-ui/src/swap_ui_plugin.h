#ifndef SWAP_UI_PLUGIN_H
#define SWAP_UI_PLUGIN_H

#include <QJsonObject>
#include <QString>
#include <QStringList>
#include <QTimer>
#include <QVariantList>
#include <functional>
#include <utility>
#include <vector>

#include "swap_ui_interface.h"
#include "LogosViewPluginBase.h"
#include "rep_swap_ui_source.h"

class LogosAPI;
class LogosObject;
class Swap;

class SwapUiPlugin : public SwapUiSimpleSource,
                     public SwapUiInterface,
                     public SwapUiViewPluginBase
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID SwapUiInterface_iid FILE "metadata.json")
    Q_INTERFACES(SwapUiInterface)

public:
    explicit SwapUiPlugin(QObject* parent = nullptr);
    ~SwapUiPlugin() override;

    QString name()    const override { return "swap_ui"; }
    QString version() const override { return "0.1.0"; }

    Q_INVOKABLE void initLogos(LogosAPI* api);

    // Slots from swap_ui.rep
    void setRole(const QString& role) override;
    void setConfigValue(const QString& key, const QString& value) override;
    void loadConfig(const QString& configJson) override;
    void loadEnvFile(const QString& path, const QString& role) override;
    bool validateConfig() override;

    void fetchBalances() override;
    void startMaker(const QString& hashlockHex) override;
    void startTaker(const QString& preimageHex) override;
    void acceptOfferAndStartTaker(const QString& offerJson) override;
    void refundLez(const QString& hashlockHex) override;
    void refundEth(const QString& swapIdHex) override;

    void initMessaging() override;
    void publishOffer() override;
    void fetchOffers() override;
    void startAutoAccept() override;
    void stopAutoAccept() override;

private:
    QString configJson() const;
    QString messagingConfigJson() const;
    void applyConfigObject(const QJsonObject& obj);
    void applyOfferObject(const QJsonObject& offer);
    bool validateConfigForAction(const QString& action,
                                 const QString& hexValue = {},
                                 const QString& hexKey = {});

    void setBusyState();
    void updateRunning();
    void clearMakerProgress();
    void clearTakerProgress();
    void addMakerProgressStep(const QString& step);
    void addTakerProgressStep(const QString& step);
    bool shouldHandleJobEvent(const QString& eventName, const QJsonObject& payload) const;
    void setResultStatus(const QString& resultJson,
                         const QString& successStatus,
                         const QString& failureStatus);
    void fetchBalancesFromLoadedEnv();
    void applyBalancesResult(const QString& resultJson);
    void handleMakerFinished(const QString& resultJson);
    void handleTakerFinished(const QString& resultJson);
    void handleAutoAcceptFinished(const QString& resultJson);
    void handleJobStartResult(const QString& role, const QString& resultJson);
    void startBackgroundServices();
    void pollMessagingStatus();
    void ensureMessagingReady(std::function<void()> continuation = {}, bool automatic = false);
    bool subscribeToSwapEvents();
    void onSwapEventArgs(const QString& eventName, const QVariantList& args);
    void onSwapEvent(const QString& eventName, const QString& payloadJson);
    void handleProgressEvent(const QString& eventName, const QJsonObject& payload);
    void handleFinishedEvent(const QString& eventName, const QJsonObject& payload);
    void addValidationError(QJsonObject& errors, const QString& key, const QString& message) const;

    // Per-swap Delivery coordination helpers (M2). See delivery-dogfooding.md.
    void coordinationStart(const QString& role, const QString& hashlockHex);
    void coordinationStop();
    void coordinationPollSwapEvents();
    void coordinationPublishTakerAccept(const QString& hashlockHex,
                                        const QString& ethSwapId);
    void coordinationAppendEvents(const QJsonArray& events);
    static QString normaliseHashlock(const QString& raw);

    static QJsonObject parseObject(const QString& json);
    static QString jsonError(const QString& json);
    static bool isErrorResult(const QString& json);
    static QString compactJson(const QJsonObject& obj);
    static QString compactJsonValue(const QJsonValue& value);
    static QString payloadFromArgs(const QVariantList& args);
    static QString jobIdFromResult(const QString& json);
    static bool isPositiveInteger(const QString& value);
    static bool isPositiveDecimal(const QString& value);
    static bool isHexBytes(const QString& value, int bytes);
    static bool isEthAddress(const QString& value);
    static bool looksLikeBase58(const QString& value);

    LogosAPI* m_logosAPI = nullptr;
    Swap* m_swap = nullptr;
    LogosObject* m_eventObject = nullptr;
    QTimer m_messagingPollTimer;
    QTimer m_coordinationPollTimer;
    bool m_messagingInitInFlight = false;
    bool m_autoMessagingEnabled = false;
    std::vector<std::function<void()>> m_pendingMessagingContinuations;
    int m_deliveryPortsShift = 0;
    QString m_loadedEnvPath;
    QString m_coordinationRole;
    bool m_coordinationTakerPublished = false;
    bool m_swapEventsSubscribed = false;
};

#endif // SWAP_UI_PLUGIN_H
