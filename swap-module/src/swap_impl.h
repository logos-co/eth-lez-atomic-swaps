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
#include <unordered_map>

extern "C" {
    #include "lib/swap_ffi.h"
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

    // Read a dotenv-style file and return its parsed contents as JSON.
    std::string loadEnv(const std::string& path);

    // Fetch on-chain ETH and LEZ balances for the configured accounts.
    std::string fetchBalances(const std::string& configJson);

    // Lifecycle for the embedded Waku messaging node.
    std::string messagingInit(const std::string& configJson);
    std::string messagingShutdown();
    std::string messagingStatus();

    // Maker offer publishing / taker offer fetching over Waku.
    std::string publishOffer(const std::string& configJson);
    std::string fetchOffers();

    // Refunds (called once a timelock has expired and the swap stalled).
    std::string refundLez(const std::string& configJson, const std::string& hashlockHex);
    std::string refundEth(const std::string& configJson, const std::string& swapIdHex);

    // ---- Long-running flows ----
    //
    // These call into the Rust orchestrator which performs multi-step on-chain
    // and over-Waku work. Today the FFI implementations are blocking and emit
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
};
