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
    // Event names: "maker.progress", "taker.progress", "maker_loop.progress".
    // Payload is the JSON string emitted by the FFI ProgressCallback.
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

private:
    // Trampoline used by the FFI ProgressCallback. Forwards (cstring, ctx) to
    // the impl's `emitEvent` under the appropriate event name.
    struct ProgressCtx {
        SwapImpl* self;
        std::string eventName;
    };
    static void progressTrampoline(const char* json, void* userData);

    // Convert a heap-allocated FFI char* to std::string and free it.
    static std::string takeAndFree(char* ptr);

    // Non-copyable, non-movable — owns FFI lifecycle.
    SwapImpl(const SwapImpl&) = delete;
    SwapImpl& operator=(const SwapImpl&) = delete;
};
