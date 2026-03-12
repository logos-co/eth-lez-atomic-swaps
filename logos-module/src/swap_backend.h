#ifndef SWAP_BACKEND_H
#define SWAP_BACKEND_H

#include <QFutureWatcher>
#include <QObject>
#include <QString>
#include <QStringList>
#include <QThreadPool>
#include "swap_ffi.h"

class SwapBackend;

struct ProgressContext {
    SwapBackend *backend;
    bool isMaker; // true = maker, false = taker
};

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

    // Balances
    Q_PROPERTY(QString ethAddress READ ethAddress NOTIFY ethAddressChanged)
    Q_PROPERTY(QString ethBalance READ ethBalance NOTIFY ethBalanceChanged)
    Q_PROPERTY(QString lezAccount READ lezAccount NOTIFY lezAccountChanged)
    Q_PROPERTY(QString lezBalance READ lezBalance NOTIFY lezBalanceChanged)

    // Maker state
    Q_PROPERTY(bool makerRunning READ makerRunning NOTIFY makerRunningChanged)
    Q_PROPERTY(QString makerCurrentStep READ makerCurrentStep NOTIFY makerCurrentStepChanged)
    Q_PROPERTY(QStringList makerProgressSteps READ makerProgressSteps NOTIFY makerProgressStepsChanged)
    Q_PROPERTY(QString makerResultJson READ makerResultJson NOTIFY makerResultJsonChanged)

    // Taker state
    Q_PROPERTY(bool takerRunning READ takerRunning NOTIFY takerRunningChanged)
    Q_PROPERTY(QString takerCurrentStep READ takerCurrentStep NOTIFY takerCurrentStepChanged)
    Q_PROPERTY(QStringList takerProgressSteps READ takerProgressSteps NOTIFY takerProgressStepsChanged)
    Q_PROPERTY(QString takerResultJson READ takerResultJson NOTIFY takerResultJsonChanged)

    // Combined running (for status bar / config panel)
    Q_PROPERTY(bool running READ running NOTIFY runningChanged)

public:
    explicit SwapBackend(QThreadPool *pool, QObject *parent = nullptr);
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

    // Maker state getters
    bool makerRunning() const { return m_makerRunning; }
    QString makerCurrentStep() const { return m_makerCurrentStep; }
    QStringList makerProgressSteps() const { return m_makerProgressSteps; }
    QString makerResultJson() const { return m_makerResultJson; }

    // Taker state getters
    bool takerRunning() const { return m_takerRunning; }
    QString takerCurrentStep() const { return m_takerCurrentStep; }
    QStringList takerProgressSteps() const { return m_takerProgressSteps; }
    QString takerResultJson() const { return m_takerResultJson; }

    // Combined
    bool running() const { return m_makerRunning || m_takerRunning; }

    Q_INVOKABLE void loadEnv();
    Q_INVOKABLE void loadConfig(const QJsonObject &config);
    Q_INVOKABLE void fetchBalances();
    Q_INVOKABLE void startMaker(const QString &hashlockHex = QString());
    Q_INVOKABLE void startTaker(const QString &preimageHex = QString());
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

    void ethAddressChanged();
    void ethBalanceChanged();
    void lezAccountChanged();
    void lezBalanceChanged();

    void makerRunningChanged();
    void makerCurrentStepChanged();
    void makerProgressStepsChanged();
    void makerResultJsonChanged();

    void takerRunningChanged();
    void takerCurrentStepChanged();
    void takerProgressStepsChanged();
    void takerResultJsonChanged();

    void runningChanged();

    void offerPublished(const QString &resultJson);
    void offersFetched(const QString &offersJson);

private:
    QByteArray configJson() const;

    // Maker state helpers
    void setMakerRunning(bool v);
    void setMakerCurrentStep(const QString &v);
    void addMakerProgressStep(const QString &v);
    void clearMakerProgress();
    void setMakerResultJson(const QString &v);

    // Taker state helpers
    void setTakerRunning(bool v);
    void setTakerCurrentStep(const QString &v);
    void addTakerProgressStep(const QString &v);
    void clearTakerProgress();
    void setTakerResultJson(const QString &v);

    void handleProgress(const QString &json, bool isMaker);

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

    // Balances
    QString m_ethAddress;
    QString m_ethBalance;
    QString m_lezAccount;
    QString m_lezBalance;

    // Maker state
    bool m_makerRunning = false;
    QString m_makerCurrentStep;
    QStringList m_makerProgressSteps;
    QString m_makerResultJson;
    QString m_publishedPreimage;

    // Taker state
    bool m_takerRunning = false;
    QString m_takerCurrentStep;
    QStringList m_takerProgressSteps;
    QString m_takerResultJson;

    QString m_nwakuUrl;

    // Separate watchers for concurrent maker + taker
    QFutureWatcher<QString> m_balanceWatcher;
    QFutureWatcher<QString> m_makerWatcher;
    QFutureWatcher<QString> m_takerWatcher;
    QFutureWatcher<QString> m_publishWatcher;
    QFutureWatcher<QString> m_fetchWatcher;

    // Progress callback contexts (stable pointers for FFI)
    ProgressContext m_makerProgressCtx;
    ProgressContext m_takerProgressCtx;
};

#endif // SWAP_BACKEND_H
