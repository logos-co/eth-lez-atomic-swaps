#ifndef SWAP_BACKEND_H
#define SWAP_BACKEND_H

#include <QFutureWatcher>
#include <QObject>
#include <QString>
#include <QStringList>
#include <QThreadPool>
#include "swap_ffi.h"

extern "C" void progressCallbackTrampoline(const char *json, void *userData);

class SwapBackend : public QObject
{
    Q_OBJECT
    friend void progressCallbackTrampoline(const char *json, void *userData);

    // Config properties (two-way bound to QML)
    Q_PROPERTY(QString ethRpcUrl READ ethRpcUrl WRITE setEthRpcUrl NOTIFY ethRpcUrlChanged)
    Q_PROPERTY(QString ethPrivateKey READ ethPrivateKey WRITE setEthPrivateKey NOTIFY ethPrivateKeyChanged)
    Q_PROPERTY(QString ethHtlcAddress READ ethHtlcAddress WRITE setEthHtlcAddress NOTIFY ethHtlcAddressChanged)
    Q_PROPERTY(QString lezSequencerUrl READ lezSequencerUrl WRITE setLezSequencerUrl NOTIFY lezSequencerUrlChanged)
    Q_PROPERTY(QString lezSigningKey READ lezSigningKey WRITE setLezSigningKey NOTIFY lezSigningKeyChanged)
    Q_PROPERTY(QString lezHtlcProgramId READ lezHtlcProgramId WRITE setLezHtlcProgramId NOTIFY lezHtlcProgramIdChanged)
    Q_PROPERTY(QString lezAmount READ lezAmount WRITE setLezAmount NOTIFY lezAmountChanged)
    Q_PROPERTY(QString ethAmount READ ethAmount WRITE setEthAmount NOTIFY ethAmountChanged)
    Q_PROPERTY(QString lezTimelockMinutes READ lezTimelockMinutes WRITE setLezTimelockMinutes NOTIFY lezTimelockMinutesChanged)
    Q_PROPERTY(QString ethTimelockMinutes READ ethTimelockMinutes WRITE setEthTimelockMinutes NOTIFY ethTimelockMinutesChanged)
    Q_PROPERTY(QString ethRecipientAddress READ ethRecipientAddress WRITE setEthRecipientAddress NOTIFY ethRecipientAddressChanged)
    Q_PROPERTY(QString lezTakerAccountId READ lezTakerAccountId WRITE setLezTakerAccountId NOTIFY lezTakerAccountIdChanged)
    Q_PROPERTY(QString pollIntervalMs READ pollIntervalMs WRITE setPollIntervalMs NOTIFY pollIntervalMsChanged)
    Q_PROPERTY(QString nwakuUrl READ nwakuUrl WRITE setNwakuUrl NOTIFY nwakuUrlChanged)

    // Role (maker / taker — set via SWAP_ROLE env var or loadConfig)
    Q_PROPERTY(QString swapRole READ swapRole CONSTANT)

    // State
    Q_PROPERTY(bool running READ running NOTIFY runningChanged)
    Q_PROPERTY(QString currentStep READ currentStep NOTIFY currentStepChanged)
    Q_PROPERTY(QStringList progressSteps READ progressSteps NOTIFY progressStepsChanged)
    Q_PROPERTY(QString resultJson READ resultJson NOTIFY resultJsonChanged)

public:
    explicit SwapBackend(QThreadPool *pool, QObject *parent = nullptr);
    ~SwapBackend() override;

    // Role getter
    QString swapRole() const { return m_swapRole; }

    // Config getters
    QString ethRpcUrl() const { return m_ethRpcUrl; }
    QString ethPrivateKey() const { return m_ethPrivateKey; }
    QString ethHtlcAddress() const { return m_ethHtlcAddress; }
    QString lezSequencerUrl() const { return m_lezSequencerUrl; }
    QString lezSigningKey() const { return m_lezSigningKey; }
    QString lezHtlcProgramId() const { return m_lezHtlcProgramId; }
    QString lezAmount() const { return m_lezAmount; }
    QString ethAmount() const { return m_ethAmount; }
    QString lezTimelockMinutes() const { return m_lezTimelockMinutes; }
    QString ethTimelockMinutes() const { return m_ethTimelockMinutes; }
    QString ethRecipientAddress() const { return m_ethRecipientAddress; }
    QString lezTakerAccountId() const { return m_lezTakerAccountId; }
    QString pollIntervalMs() const { return m_pollIntervalMs; }
    QString nwakuUrl() const { return m_nwakuUrl; }

    // Config setters
    void setEthRpcUrl(const QString &v);
    void setEthPrivateKey(const QString &v);
    void setEthHtlcAddress(const QString &v);
    void setLezSequencerUrl(const QString &v);
    void setLezSigningKey(const QString &v);
    void setLezHtlcProgramId(const QString &v);
    void setLezAmount(const QString &v);
    void setEthAmount(const QString &v);
    void setLezTimelockMinutes(const QString &v);
    void setEthTimelockMinutes(const QString &v);
    void setEthRecipientAddress(const QString &v);
    void setLezTakerAccountId(const QString &v);
    void setPollIntervalMs(const QString &v);
    void setNwakuUrl(const QString &v);

    // State getters
    bool running() const { return m_running; }
    QString currentStep() const { return m_currentStep; }
    QStringList progressSteps() const { return m_progressSteps; }
    QString resultJson() const { return m_resultJson; }

    Q_INVOKABLE void loadEnv();
    Q_INVOKABLE void loadConfig(const QJsonObject &config);
    Q_INVOKABLE void startMaker(const QString &preimageHex = QString());
    Q_INVOKABLE void startTaker(const QString &hashlockHex);
    Q_INVOKABLE void refundLez(const QString &hashlockHex);
    Q_INVOKABLE void refundEth(const QString &swapIdHex);
    Q_INVOKABLE void publishOffer();
    Q_INVOKABLE void fetchOffers();

signals:
    void ethRpcUrlChanged();
    void ethPrivateKeyChanged();
    void ethHtlcAddressChanged();
    void lezSequencerUrlChanged();
    void lezSigningKeyChanged();
    void lezHtlcProgramIdChanged();
    void lezAmountChanged();
    void ethAmountChanged();
    void lezTimelockMinutesChanged();
    void ethTimelockMinutesChanged();
    void ethRecipientAddressChanged();
    void lezTakerAccountIdChanged();
    void pollIntervalMsChanged();
    void nwakuUrlChanged();

    void runningChanged();
    void currentStepChanged();
    void progressStepsChanged();
    void resultJsonChanged();

    void offerPublished(const QString &resultJson);
    void offersFetched(const QString &offersJson);

private:
    QByteArray configJson() const;
    void setRunning(bool v);
    void setCurrentStep(const QString &v);
    void addProgressStep(const QString &v);
    void clearProgress();
    void setResultJson(const QString &v);

    void handleProgress(const QString &json);

    // Dedicated thread pool (not global)
    QThreadPool *m_threadPool;

    // Role
    QString m_swapRole;

    // Config fields
    QString m_ethRpcUrl;
    QString m_ethPrivateKey;
    QString m_ethHtlcAddress;
    QString m_lezSequencerUrl;
    QString m_lezSigningKey;
    QString m_lezHtlcProgramId;
    QString m_lezAmount;
    QString m_ethAmount;
    QString m_lezTimelockMinutes;
    QString m_ethTimelockMinutes;
    QString m_ethRecipientAddress;
    QString m_lezTakerAccountId;
    QString m_pollIntervalMs;
    QString m_nwakuUrl;

    // State
    bool m_running = false;
    QString m_currentStep;
    QStringList m_progressSteps;
    QString m_resultJson;
    QString m_publishedPreimage;

    QFutureWatcher<QString> m_watcher;
    QFutureWatcher<QString> m_publishWatcher;
    QFutureWatcher<QString> m_fetchWatcher;
};

#endif // SWAP_BACKEND_H
