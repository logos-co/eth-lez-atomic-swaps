#pragma once
//
// LEZ <> ETH atomic swap module — universal C++ implementation.
//
// This is a thin wrapper around the Rust swap-ffi cdylib (libswap_ffi.{dylib,so}).
// All methods accept and return JSON strings, matching the underlying FFI's ABI.
//
// The build pipeline (logos-cpp-generator --from-header) consumes this file to
// produce the Qt plugin glue. As such, the public API must use only these types:
//   std::string, bool, int64_t, uint64_t, double, void, std::vector<T>
//
// See AGENTS.md and .cursor/rules/logos.mdc for the full type-mapping table.
//

#include <string>
#include <vector>
#include <cstdint>
#include <functional>
#include <atomic>
#include <memory>
#include <mutex>
#include <thread>
#include <unordered_map>

extern "C" {
    #include "swap_ffi.h"
}

class SwapImpl {
public:
    SwapImpl();
    ~SwapImpl();

    // Event emitter — auto-detected by logos-cpp-generator and wired to
    // LogosProviderBase::emitEvent. Used to push progress updates from
    // long-running maker/taker flows back to UI subscribers.
    //
    // Event names:
    // - maker.progress / maker.finished
    // - taker.progress / taker.finished
    // - maker_loop.progress / maker_loop.finished
    //
    // Payload shape:
    // {"job_id":"...","role":"maker|taker|maker_loop","step":"...",
    //  "data":{...},"result":{...},"error":null|string,"timestamp_ms":...}
    std::function<void(const std::string& eventName, const std::string& data)> emitEvent;

    // ---- Synchronous queries ----
    // Each returns the JSON string produced by the underlying FFI call.

    // Per-profile, host-owned persistence root for THIS swap module instance
    // (shape `<basecamp>/module_data/swap/<id>/`) — or an empty string when the
    // module runs outside a persistence-provisioning host (unit tests / lgpd).
    //
    // The Logos runtime stamps this path onto the module's LogosAPI as the
    // `instancePersistencePath` property before any method is dispatched (see
    // logos-liblogos runtime_qt/host/module_initializer.cpp). The generated
    // provider's onInit hands that LogosAPI to the delivery adapter, which is
    // where persistenceRoot() reads the property from — no LogosModuleContext
    // mixin (that lives in a newer cpp-sdk than this module builds against).
    //
    // Exists so the out-of-process swap_ui plugin can anchor its config.json +
    // receipts.jsonl inside the ACTIVE Basecamp profile. swap_ui is a ui_qml
    // module hosted in a separate ui-host process that never receives
    // LOGOS_USER_DIR and has no host identity of its own, so its Qt
    // AppDataLocation fallback lands in a `Logos/ui-host/` tree SHARED across
    // every Basecamp profile on the machine — leaking two private keys across
    // profiles (issue #99). By querying this in-process core module (which does
    // get a correct per-profile path) over the existing swap interface, swap_ui
    // writes under `<root>/swap_ui/` instead. Empty return means "no
    // host-provisioned root", and swap_ui falls back to LOGOS_USER_DIR / its
    // legacy path.
    std::string persistenceRoot();

    // Read a dotenv-style file and return its parsed contents as JSON.
    std::string loadEnv(const std::string& path);

    // Canonical LEZ HTLC program ID baked into the Rust library, as a 64-char
    // hex string — or an empty string if this build doesn't have it compiled
    // in (the ID is gated behind the FFI's `demo` feature). Lets the UI default
    // the maker's program-ID field from a single canonical source.
    std::string defaultLezHtlcProgramId();

    // Fetch on-chain ETH and LEZ balances for the configured accounts.
    std::string fetchBalances(const std::string& configJson);

    // Load config from an env file internally, then fetch balances without
    // exposing secret-bearing config JSON through module method arguments.
    std::string fetchBalancesFromEnv(const std::string& path);

    // Lifecycle for the Delivery-backed messaging node.
    std::string messagingInit(const std::string& configJson);
    std::string messagingShutdown();
    std::string messagingStatus();

    // Maker offer publishing / taker offer fetching over Delivery.
    std::string publishOffer(const std::string& configJson);
    std::string fetchOffers();

    // RFQ (request-for-quote): publish an anonymous offer-request so live makers
    // respond with their current offer immediately. The maker's fallback
    // heartbeat remains the reliable baseline; this just accelerates the first
    // fill. Takes no arguments and carries no identity — see
    // swap_delivery_adapter.h.
    std::string publishOfferRequest();

    // Per-swap coordination over Delivery on /atomic-swaps/1/swap-<hashlock>/json.
    // Layered on top of the existing on-chain ETH/LEZ flow: the maker
    // subscribes after detecting the on-chain ETH lock, the taker publishes
    // a SwapAccept after locking ETH, and both sides drain coordination
    // events with fetchSwapEvents. The orchestrator still relies on
    // on-chain watchers; these calls expose the M2 Delivery channel.
    std::string subscribeSwap(const std::string& hashlockHex);
    std::string unsubscribeSwap(const std::string& hashlockHex);
    std::string publishSwapAccept(const std::string& configJson);
    std::string fetchSwapEvents(const std::string& hashlockHex);

    // Refunds (called once a timelock has expired and the swap stalled).
    std::string refundLez(const std::string& configJson, const std::string& hashlockHex);
    std::string refundEth(const std::string& configJson, const std::string& swapIdHex);

    // ---- Onboarding (generate/init/fund) ----
    //
    // Replaces the worst part of first-run setup — hand-typing two private
    // keys and two long account IDs into the Config tab (#87/#91) — with
    // buttons. Thin wrappers over the new swap-ffi onboarding surface, itself
    // a thin wrapper over `src/lez/onboard.rs` (lifted from `lez-mcp` in #77
    // and live-verified against the public testnet). No new crypto here.

    // Generate a fresh random ETH signing key. No network call. Returns JSON
    // {"private_key":"0x...","address":"0x..."}. The address is what a taker
    // should publish as its own eth_recipient_address.
    std::string generateEthKey();

    // Generate a fresh LEZ signing key + its derived account ID. No network
    // call — the account does not exist on-chain until lezEnsureInitialized
    // (or startLezFundingJob, which calls that first) runs. Returns JSON
    // {"signing_key":"<64-char hex>","account_id":"<base58>"}.
    std::string generateLezAccount();

    // Idempotently ensure a LEZ account is initialized (owned by the
    // authenticated_transfer program) on-chain. Safe to call at any time.
    // startLezFundingJob below also calls this first internally, so a caller
    // that skips straight to funding still gets init-before-claim — the
    // sequencer silently drops pinata claims against a never-initialized
    // account, and that ordering guarantee lives in the Rust layer, not in
    // whichever order a UI happens to call these two. Returns JSON
    // {"outcome":"AlreadyInitialized"} or
    // {"outcome":"Initialized","data":{"tx_hash":"..."}}.
    std::string lezEnsureInitialized(const std::string& sequencerUrl, const std::string& signingKeyHex);

    // Start a background job that ensures the account is initialized, then
    // claims from the native pinata faucet until its balance reaches
    // targetLez (a decimal LEZ string, e.g. "150" — one claim; see the
    // funding-target rationale in swap_impl.cpp). Emits progress via
    // lez_setup.progress / lez_setup.finished, same enriched job-payload
    // shape as the maker/taker/maker_loop jobs; `data` carries the raw
    // FundingProgress event from src/lez/onboard.rs (Initializing,
    // CheckingBalance, Claimed, ClaimFailed, TargetReached, ...). Returns a
    // JSON job descriptor immediately, same shape as startMakerJob etc.
    //
    // NOTE: unlike the maker/taker jobs, this cannot be interrupted mid-flight
    // — stopJob() marks it "cancelling" in the job status but the underlying
    // claim_to_target loop (a plain blocking Rust future with no cancellation
    // token) runs to completion regardless. Acceptable for a single ~150 LEZ
    // claim; a follow-up would thread a cancel flag through if a target ever
    // needs many claims.
    std::string startLezFundingJob(const std::string& sequencerUrl, const std::string& signingKeyHex, const std::string& targetLez);

    // Ask the in-house Sepolia drip faucet for test ETH (PoC — see
    // README-poc.md and eth-faucet/). Fetches a proof-of-work challenge from
    // `faucetUrl`, solves it (CPU-bound, seconds), and posts the answer; the
    // faucet sends the drip and waits for its receipt before answering, so a
    // successful return means the ETH is in a block, not merely submitted.
    //
    // BLOCKING for as long as the whole round trip takes (PoW solve plus a
    // Sepolia inclusion wait — tens of seconds is normal), which is well past
    // the generated *Async wrapper's default 20s Timeout: callers must pass an
    // explicit longer Timeout or the answer arrives as an empty QString and
    // reads as a failure (the shape of issue #171). See
    // SwapUiPlugin::setupRequestTestEth.
    //
    // Returns JSON {"outcome":"Dripped","tx_hash":...,"amount_eth":...,
    // "chain_id":...} or {"error":"<sentence for the user>","code":"<tag>"}.
    // The faucet authors its own refusal sentences (only it knows how long a
    // cooldown has left), so the UI shows `error` verbatim.
    std::string faucetRequestEth(const std::string& faucetUrl, const std::string& address);

    // ---- Long-running flows ----
    //
    // These call into the Rust orchestrator which performs multi-step on-chain
    // and Delivery work. Today the FFI implementations are blocking and emit
    // progress via a C callback. We expose them as blocking calls here for now
    // and route the C callback into `emitEvent`. A follow-up should make these
    // non-blocking (spawn worker thread, return swap-id immediately).
    //
    // For maker/taker, when `hashlockHex` / `preimageHex` is empty, the FFI
    // generates a fresh one. The returned JSON contains the final outcome.

    std::string runMaker(const std::string& configJson, const std::string& hashlockHex);
    std::string runTaker(const std::string& configJson, const std::string& preimageHex);

    // Run an auto-accept maker loop until stopMakerLoop is invoked.
    // Emits per-iteration progress via `emitEvent`.
    std::string runMakerLoop(const std::string& configJson);
    void stopMakerLoop();

    // ---- Async job API for UI clients ----
    //
    // These start worker threads and return immediately with a JSON job
    // descriptor. The final result is delivered by the corresponding
    // *.finished event and can also be read via jobStatus(jobId).
    std::string startMakerJob(const std::string& configJson, const std::string& hashlockHex);
    std::string startTakerJob(const std::string& configJson, const std::string& preimageHex);
    std::string startMakerLoopJob(const std::string& configJson);
    std::string stopJob(const std::string& jobId);
    std::string jobStatus(const std::string& jobId);

private:
    struct EmitterState;
    struct JobState;

    // Trampoline used by the FFI ProgressCallback. Wraps raw progress JSON in
    // the enriched job payload shape before emitting.
    struct ProgressCtx {
        std::shared_ptr<JobState> job;
        std::shared_ptr<EmitterState> emitter;
        std::string progressEventName;
    };
    static void progressTrampoline(const char* json, void* userData);

    // Convert a heap-allocated FFI char* to std::string and free it.
    static std::string takeAndFree(char* ptr);

    std::string startJob(const std::string& role,
                         const std::string& configJson,
                         const std::string& secretHex);
    std::shared_ptr<JobState> activeJobForRoleLocked(const std::string& role) const;
    void setActiveJobForRoleLocked(const std::string& role, const std::shared_ptr<JobState>& job);
    std::string runBlockingJob(const std::string& role,
                               const std::string& configJson,
                               const std::string& secretHex);

    static std::string newJobId(const std::string& role, uint64_t id);
    static std::string progressEventName(const std::string& role);
    static std::string finishedEventName(const std::string& role);
    static std::string normalizeRole(const std::string& role);
    static bool isTerminalStatus(const std::string& status);
    static int64_t timestampMs();
    static void safeEmit(const std::shared_ptr<EmitterState>& emitter,
                         const std::string& eventName,
                         const std::string& payload);
    static std::string progressPayload(const std::shared_ptr<JobState>& job,
                                       const std::string& rawProgressJson);
    static std::string finishedPayload(const std::shared_ptr<JobState>& job);
    static std::string jobJson(const std::shared_ptr<JobState>& job);
    static std::string errorJson(const std::string& error);
    static void setJobFinished(const std::shared_ptr<JobState>& job,
                               const std::string& resultJson);

    // Non-copyable, non-movable — owns FFI lifecycle.
    SwapImpl(const SwapImpl&) = delete;
    SwapImpl& operator=(const SwapImpl&) = delete;

    std::shared_ptr<EmitterState> m_emitter;
    std::atomic<uint64_t> m_nextJobId{1};
    mutable std::mutex m_jobsMutex;
    std::unordered_map<std::string, std::shared_ptr<JobState>> m_jobs;
    std::shared_ptr<JobState> m_makerJob;
    std::shared_ptr<JobState> m_takerJob;
    std::shared_ptr<JobState> m_makerLoopJob;
    std::shared_ptr<JobState> m_lezSetupJob;
    std::mutex m_workersMutex;
    std::vector<std::thread> m_workers;
};
