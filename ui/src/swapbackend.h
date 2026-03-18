#ifndef SWAPBACKEND_H
#define SWAPBACKEND_H

#include <QFutureWatcher>
#include <QObject>
#include <QString>
#include <QStringList>
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
    Q_PROPERTY(QString lezWalletHome READ lezWalletHome WRITE setLezWalletHome NOTIFY lezWalletHomeChanged)
    Q_PROPERTY(QString lezAccountId READ lezAccountId WRITE setLezAccountId NOTIFY lezAccountIdChanged)
    Q_PROPERTY(QString lezHtlcProgramId READ lezHtlcProgramId WRITE setLezHtlcProgramId NOTIFY lezHtlcProgramIdChanged)
    Q_PROPERTY(QString lezAmount READ lezAmount WRITE setLezAmount NOTIFY lezAmountChanged)
    Q_PROPERTY(QString ethAmount READ ethAmount WRITE setEthAmount NOTIFY ethAmountChanged)
    Q_PROPERTY(QString lezTimelockMinutes READ lezTimelockMinutes WRITE setLezTimelockMinutes NOTIFY lezTimelockMinutesChanged)
    Q_PROPERTY(QString ethTimelockMinutes READ ethTimelockMinutes WRITE setEthTimelockMinutes NOTIFY ethTimelockMinutesChanged)
    Q_PROPERTY(QString ethRecipientAddress READ ethRecipientAddress WRITE setEthRecipientAddress NOTIFY ethRecipientAddressChanged)
    Q_PROPERTY(QString lezTakerAccountId READ lezTakerAccountId WRITE setLezTakerAccountId NOTIFY lezTakerAccountIdChanged)
    Q_PROPERTY(QString pollIntervalMs READ pollIntervalMs WRITE setPollIntervalMs NOTIFY pollIntervalMsChanged)
    Q_PROPERTY(QString nwakuUrl READ nwakuUrl WRITE setNwakuUrl NOTIFY nwakuUrlChanged)

    // Role (maker / taker — set via SWAP_ROLE env var)
    Q_PROPERTY(QString swapRole READ swapRole CONSTANT)

    // Balances
    Q_PROPERTY(QString ethAddress READ ethAddress NOTIFY ethAddressChanged)
    Q_PROPERTY(QString ethBalance READ ethBalance NOTIFY ethBalanceChanged)
    Q_PROPERTY(QString lezAccount READ lezAccount NOTIFY lezAccountChanged)
    Q_PROPERTY(QString lezBalance READ lezBalance NOTIFY lezBalanceChanged)

    // State
    Q_PROPERTY(bool running READ running NOTIFY runningChanged)
    Q_PROPERTY(QString currentStep READ currentStep NOTIFY currentStepChanged)
    Q_PROPERTY(QStringList progressSteps READ progressSteps NOTIFY progressStepsChanged)
    Q_PROPERTY(QString resultJson READ resultJson NOTIFY resultJsonChanged)

    // Auto-accept state
    Q_PROPERTY(bool autoAcceptRunning READ autoAcceptRunning NOTIFY autoAcceptRunningChanged)
    Q_PROPERTY(int autoAcceptCompleted READ autoAcceptCompleted NOTIFY autoAcceptCompletedChanged)
    Q_PROPERTY(int autoAcceptFailed READ autoAcceptFailed NOTIFY autoAcceptFailedChanged)
    Q_PROPERTY(int autoAcceptIteration READ autoAcceptIteration NOTIFY autoAcceptIterationChanged)
    Q_PROPERTY(QStringList swapHistory READ swapHistory NOTIFY swapHistoryChanged)

public:
    explicit SwapBackend(QObject *parent = nullptr);
    ~SwapBackend() override;

    // Role getter
    QString swapRole() const { return m_swapRole; }

    // Balance getters
    QString ethAddress() const { return m_ethAddress; }
    QString ethBalance() const { return m_ethBalance; }
    QString lezAccount() const { return m_lezAccount; }
    QString lezBalance() const { return m_lezBalance; }

    // Config getters
    QString ethRpcUrl() const { return m_ethRpcUrl; }
    QString ethPrivateKey() const { return m_ethPrivateKey; }
    QString ethHtlcAddress() const { return m_ethHtlcAddress; }
    QString lezSequencerUrl() const { return m_lezSequencerUrl; }
    QString lezSigningKey() const { return m_lezSigningKey; }
    QString lezWalletHome() const { return m_lezWalletHome; }
    QString lezAccountId() const { return m_lezAccountId; }
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
    void setLezWalletHome(const QString &v);
    void setLezAccountId(const QString &v);
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
    bool running() const { return m_running || m_autoAcceptRunning; }
    QString currentStep() const { return m_currentStep; }
    QStringList progressSteps() const { return m_progressSteps; }
    QString resultJson() const { return m_resultJson; }

    // Auto-accept getters
    bool autoAcceptRunning() const { return m_autoAcceptRunning; }
    int autoAcceptCompleted() const { return m_autoAcceptCompleted; }
    int autoAcceptFailed() const { return m_autoAcceptFailed; }
    int autoAcceptIteration() const { return m_autoAcceptIteration; }
    QStringList swapHistory() const { return m_swapHistory; }

    Q_INVOKABLE void loadEnv();
    Q_INVOKABLE void fetchBalances();
    Q_INVOKABLE void startMaker(const QString &hashlockHex = QString());
    Q_INVOKABLE void startTaker(const QString &preimageHex = QString());
    Q_INVOKABLE void refundLez(const QString &hashlockHex);
    Q_INVOKABLE void refundEth(const QString &swapIdHex);
    Q_INVOKABLE void publishOffer();
    Q_INVOKABLE void fetchOffers();
    Q_INVOKABLE void startAutoAccept();
    Q_INVOKABLE void stopAutoAccept();

signals:
    void ethRpcUrlChanged();
    void ethPrivateKeyChanged();
    void ethHtlcAddressChanged();
    void lezSequencerUrlChanged();
    void lezSigningKeyChanged();
    void lezWalletHomeChanged();
    void lezAccountIdChanged();
    void lezHtlcProgramIdChanged();
    void lezAmountChanged();
    void ethAmountChanged();
    void lezTimelockMinutesChanged();
    void ethTimelockMinutesChanged();
    void ethRecipientAddressChanged();
    void lezTakerAccountIdChanged();
    void pollIntervalMsChanged();
    void nwakuUrlChanged();

    void ethAddressChanged();
    void ethBalanceChanged();
    void lezAccountChanged();
    void lezBalanceChanged();

    void runningChanged();
    void currentStepChanged();
    void progressStepsChanged();
    void resultJsonChanged();

    void autoAcceptRunningChanged();
    void autoAcceptCompletedChanged();
    void autoAcceptFailedChanged();
    void autoAcceptIterationChanged();
    void swapHistoryChanged();

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

    // Role
    QString m_swapRole;

    // Config fields
    QString m_ethRpcUrl;
    QString m_ethPrivateKey;
    QString m_ethHtlcAddress;
    QString m_lezSequencerUrl;
    QString m_lezSigningKey;
    QString m_lezWalletHome;
    QString m_lezAccountId;
    QString m_lezHtlcProgramId;
    QString m_lezAmount;
    QString m_ethAmount;
    QString m_lezTimelockMinutes;
    QString m_ethTimelockMinutes;
    QString m_ethRecipientAddress;
    QString m_lezTakerAccountId;
    QString m_pollIntervalMs;
    QString m_nwakuUrl;

    // Balances
    QString m_ethAddress;
    QString m_ethBalance;
    QString m_lezAccount;
    QString m_lezBalance;

    // State
    bool m_running = false;
    QString m_currentStep;
    QStringList m_progressSteps;
    QString m_resultJson;
    QString m_publishedPreimage;

    // Auto-accept state
    bool m_autoAcceptRunning = false;
    int m_autoAcceptCompleted = 0;
    int m_autoAcceptFailed = 0;
    int m_autoAcceptIteration = 0;
    QStringList m_swapHistory;

    QFutureWatcher<QString> m_watcher;
    QFutureWatcher<QString> m_balanceWatcher;
    QFutureWatcher<QString> m_publishWatcher;
    QFutureWatcher<QString> m_fetchWatcher;
    QFutureWatcher<QString> m_autoAcceptWatcher;
};

#endif // SWAPBACKEND_H
