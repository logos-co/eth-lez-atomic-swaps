#ifndef SWAP_UI_PLUGIN_H
#define SWAP_UI_PLUGIN_H

#include <QJsonArray>
#include <QJsonObject>
#include <QString>
#include <QStringList>
#include <QTimer>
#include <QVariantList>
#include <csignal>
#include <functional>
#include <utility>
#include <vector>

#include "swap_ui_interface.h"
#include "balance_refresh_coordinator.h"
#include "LogosViewPluginBase.h"
#include "rep_swap_ui_source.h"

class LogosAPI;
class LogosObject;
class QSocketNotifier;
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
    QString version() const override { return "0.3.4"; }

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
    void publishOfferRequest() override;
    void startAutoAccept() override;
    void stopAutoAccept() override;

    void refreshHistory() override;
    void clearHistory() override;

    void resetConfig() override;

    void setupGenerateEthKey() override;
    void setupGenerateLezAccount() override;
    void setupStartFunding() override;

private:
    QString configJson() const;
    QString messagingConfigJson() const;
    void applyConfigObject(const QJsonObject& obj);
    // Adopt an accepted offer's ECONOMIC terms into the taker config. Returns
    // false (and surfaces a UI error) WITHOUT mutating config if the offer
    // names a non-canonical swap venue (P0 fund-theft guard) — see
    // offerNamesCanonicalVenue. Never copies the offer's eth_htlc_address /
    // lez_htlc_program_id: those stay pinned to the app's canonical values.
    bool applyOfferObject(const QJsonObject& offer);
    void applyDefaultConfig();

    // The canonical LEZ HTLC program ID: the value baked into the Rust library
    // (resolved async via defaultLezHtlcProgramIdAsync and cached below). Falls
    // back to the current config field when the async value has not arrived yet
    // — on the honest path both are the pinned default. This is the trusted
    // value an offer's advertised lez_htlc_program_id must match.
    QString canonicalLezHtlcProgramId() const;
    // P0: true iff the offer's advertised swap venue (eth_htlc_address,
    // lez_htlc_program_id) matches the app's canonical/pinned values. On a
    // mismatch, `reason` (if non-null) is filled with a UI-ready explanation.
    bool offerNamesCanonicalVenue(const QJsonObject& offer, QString* reason) const;
    bool validateConfigForAction(const QString& action,
                                 const QString& hexValue = {},
                                 const QString& hexKey = {});

    // Browsing the offer board only needs the network endpoints/constants
    // used to reach Delivery and read offers (fetchOffersAsync itself takes
    // no config at all — see fetchOffers()) — not the taker's own
    // credentials, trade amounts, or timelocks. validateForTrade is the full
    // validateConfig() check every other action (accept, publish,
    // auto-accept, refund) still requires. See feat/browse-before-config.
    bool validateForBrowse() const;
    bool validateForTrade();

    // Config persistence (config.json under the per-profile swap_ui dir; see
    // writableModuleDir()). Holds two private keys (eth_private_key,
    // lez_signing_key) — written 0600, atomic temp+rename.
    //
    // scheduleConfigSave() writes SYNCHRONOUSLY on every call (the payload is
    // a few hundred bytes; there is no perf reason to defer it). This is
    // deliberate: the out-of-process ui-host that hosts this plugin tears
    // down via a raw OS termination signal on Quit, not a graceful Qt
    // shutdown — ~SwapUiPlugin() never runs, and a debounce timer pending at
    // the moment of that signal would have silently lost the edit. See
    // installSignalHandlers()/handleUnixSignal() below for the belt-and-braces
    // second line of defense.
    // Path resolution (issue #99). swap_ui runs in an out-of-process ui-host
    // that receives no LOGOS_USER_DIR under a default Basecamp launch, so its
    // Qt AppDataLocation fallback lands in a `Logos/ui-host/` tree SHARED
    // across every profile on the machine — leaking config.json's two private
    // keys (eth_private_key, lez_signing_key). writableModuleDir() resolves the
    // correct per-profile directory instead: (a) LOGOS_USER_DIR when explicitly
    // set (explicit --user-dir launches export it and that path is already
    // profile-correct and convention-matching — preferred so it is never
    // disturbed), else (b) the swap CORE module's instancePersistencePath()
    // (per-profile; requested over the swap interface — see
    // requestPersistenceRoot()), which is the fix for a default launch where
    // LOGOS_USER_DIR is unset. It NEVER returns the legacy shared tree.
    // legacyModuleDir() is a READ-ONLY migration fallback consulted by
    // configReadPath()/receiptsReadPath() only when the writable location has
    // nothing yet. writableModuleDir() is empty while the per-profile root is
    // still unknown (default launch, core connection not up), in which case
    // writes are deferred/buffered rather than sent to the shared tree.
    QString writableModuleDir() const;
    QString legacyModuleDir() const;
    QString configReadPath() const;
    QString receiptsReadPath() const;
    void loadConfigFromDisk();
    void scheduleConfigSave();
    void saveConfigToDisk();

    // The per-profile persistence root comes from the swap CORE module, which
    // — unlike this UI plugin — receives a per-profile context from the host.
    // That core connection may come up AFTER the plugin starts, so the root is
    // requested asynchronously from initLogos and applied here when it arrives:
    // config is re-resolved (a profile-local copy wins; otherwise the init-time
    // migration-read stays in memory and is flushed into the profile) and any
    // writes deferred/buffered while the root was unknown are flushed.
    void requestPersistenceRoot();
    void onPersistenceRootResolved(const QString& root);
    void flushPendingReceipts();

    // Termination-signal handling (belt-and-braces flush; the primary fix is
    // the synchronous save above). SIGTERM/SIGINT can arrive at any point in
    // this out-of-process host's lifetime, so only async-signal-safe work
    // (write() to a self-pipe) happens in the raw handler; the actual flush
    // runs on the main thread's event loop via a QSocketNotifier, the
    // standard Qt pattern for this. The previous disposition for each signal
    // is captured and re-invoked after our flush so we don't swallow
    // ui-host's own shutdown handling.
    void installSignalHandlers();
    void handleUnixSignal();
    static void unixSignalHandler(int sig);

    void setBusyState();
    // Start a bounded post-swap settle-poll (see BalanceRefreshCoordinator):
    // snapshot the current balance and keep refreshing until it changes or the
    // attempt cap is hit, so the header reflects a claim that confirms shortly
    // after the swap "finishes" without a manual Refresh.
    void beginBalanceSettle();
    std::string balanceSnapshotKey() const;
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
    void requestAutomaticBalanceRefresh();
    void completeBalanceRefresh(const QString& resultJson);
    void applyBalancesResult(const QString& resultJson);
    void handleMakerFinished(const QString& resultJson);
    void handleTakerFinished(const QString& resultJson);
    void handleAutoAcceptFinished(const QString& resultJson);
    void handleJobStartResult(const QString& role, const QString& resultJson);
    void handleSetupFundingFinished(const QString& resultJson);
    void startBackgroundServices();
    void pollMessagingStatus();
    void ensureMessagingReady(std::function<void()> continuation = {}, bool automatic = false);
    bool subscribeToSwapEvents();
    void onSwapEventArgs(const QString& eventName, const QVariantList& args);
    void onSwapEvent(const QString& eventName, const QString& payloadJson);
    void handleProgressEvent(const QString& eventName, const QJsonObject& payload);
    void handleFinishedEvent(const QString& eventName, const QJsonObject& payload);
    void addValidationError(QJsonObject& errors, const QString& key, const QString& message) const;

    // Receipt capture + durable JSONL journal (receipt cards, PR2).
    // Evidence objects accumulate per-swap data from the progress events the
    // plugin already receives; a receipt is assembled and journaled at each
    // completion point (finished events, maker-loop per-swap completions,
    // successful manual refunds).
    void snapshotEvidenceFromConfig(QJsonObject& evidence) const;
    void beginTakerEvidence(const QJsonObject& offerContext);
    void beginMakerEvidence(bool stampStart);
    void captureTakerProgressEvidence(const QString& step,
                                      const QJsonObject& data,
                                      const QJsonObject& payload);
    void captureMakerProgressEvidence(const QString& step,
                                      const QJsonObject& data,
                                      const QJsonObject& payload);
    QJsonObject buildReceipt(const QString& role,
                             const QJsonObject& result,
                             const QJsonObject& evidence) const;
    void journalReceipt(const QJsonObject& receipt);
    void loadReceiptsFromDisk();
    void publishReceiptsProp();
    static qint64 eventTimestampMs(const QJsonObject& payload);

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
    QTimer m_balanceSettleTimer;
    QSocketNotifier* m_signalNotifier = nullptr;
    bool m_messagingInitInFlight = false;
    bool m_autoMessagingEnabled = false;
    std::vector<std::function<void()>> m_pendingMessagingContinuations;
    int m_deliveryPortsShift = 0;
    QString m_loadedEnvPath;
    BalanceRefreshCoordinator m_balanceRefreshCoordinator;
    QString m_coordinationRole;
    bool m_coordinationTakerPublished = false;
    bool m_swapEventsSubscribed = false;

    // Persistence-root state (issue #99). m_persistenceRoot is the per-profile
    // root the swap CORE module reports (empty until it arrives over the async
    // connection). Writes attempted before it is known are deferred
    // (m_pendingConfigSave) or buffered (m_pendingReceiptLines) and flushed by
    // onPersistenceRootResolved(), so nothing ever reaches the shared ui-host
    // tree.
    QString m_persistenceRoot;
    bool m_pendingConfigSave = false;
    QStringList m_pendingReceiptLines;

    // Receipt journal state. m_receipts is the surfaced in-memory list
    // (newest first, capped); the evidence objects hold the in-flight swap's
    // accumulated context per role. m_pendingOfferContext carries the
    // accepted offer from acceptOfferAndStartTaker into startTaker.
    QJsonArray m_receipts;
    QJsonObject m_takerEvidence;
    QJsonObject m_makerEvidence;
    QJsonObject m_pendingOfferContext;

    // Cached canonical LEZ HTLC program ID, populated from the Rust library via
    // defaultLezHtlcProgramIdAsync (see initLogos). Used to reject offers that
    // advertise a non-canonical LEZ program (P0 fund-theft guard).
    QString m_canonicalLezHtlcProgramId;
};

#endif // SWAP_UI_PLUGIN_H
