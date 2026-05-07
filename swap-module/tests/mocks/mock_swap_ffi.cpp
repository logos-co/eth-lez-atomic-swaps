#include "../../lib/swap_ffi.h"

#include <cstdlib>
#include <cstring>
#include <atomic>
#include <chrono>
#include <thread>

namespace {

char* copyJson(const char* json)
{
    const auto len = std::strlen(json);
    auto* out = static_cast<char*>(std::malloc(len + 1));
    std::memcpy(out, json, len + 1);
    return out;
}

std::atomic_bool makerLoopCancel{false};

} // namespace

extern "C" {

char* swap_ffi_load_env(const char*)
{
    return copyJson(R"({"eth_rpc_url":"ws://127.0.0.1:8545","eth_timelock_minutes":"10","lez_timelock_minutes":"5","poll_interval_ms":"2000"})");
}

char* swap_ffi_run_maker(const char*, const char*, ProgressCallback cb, void* user_data)
{
    if (cb) {
        cb(R"({"step":"WaitingForEthLock"})", user_data);
        cb(R"({"step":"EthLockDetected","data":{"swap_id":"0xabc"}})", user_data);
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    return copyJson(R"({"ok":true,"method":"runMaker"})");
}

char* swap_ffi_run_taker(const char*, const char*, ProgressCallback cb, void* user_data)
{
    if (cb) {
        cb(R"({"step":"PreimageGenerated","data":{"hashlock":"abc"}})", user_data);
        cb(R"({"step":"EthLocked","data":{"swap_id":"0xdef"}})", user_data);
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    return copyJson(R"({"ok":true,"method":"runTaker"})");
}

char* swap_ffi_messaging_init(const char*)
{
    return copyJson(R"({"ok":true,"method":"messagingInit"})");
}

char* swap_ffi_messaging_shutdown()
{
    return copyJson(R"({"ok":true,"method":"messagingShutdown"})");
}

char* swap_ffi_messaging_status()
{
    return copyJson(R"({"ok":true,"method":"messagingStatus"})");
}

char* swap_ffi_publish_offer(const char*)
{
    return copyJson(R"({"ok":true,"method":"publishOffer"})");
}

char* swap_ffi_fetch_offers()
{
    return copyJson(R"({"ok":true,"method":"fetchOffers"})");
}

char* swap_ffi_refund_lez(const char*, const char*)
{
    return copyJson(R"({"ok":true,"method":"refundLez"})");
}

char* swap_ffi_refund_eth(const char*, const char*)
{
    return copyJson(R"({"ok":true,"method":"refundEth"})");
}

char* swap_ffi_fetch_balances(const char*)
{
    return copyJson(R"({"ok":true,"method":"fetchBalances"})");
}

char* swap_ffi_run_maker_loop(const char*, ProgressCallback cb, void* user_data)
{
    makerLoopCancel.store(false);
    if (cb) {
        cb(R"({"step":"AutoAcceptStarted"})", user_data);
        cb(R"({"step":"AutoAcceptIteration","data":{"iteration":1}})", user_data);
    }
    for (int i = 0; i < 20 && !makerLoopCancel.load(); ++i) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    if (cb) {
        cb(R"({"step":"AutoAcceptStopped","data":{"total_completed":0,"total_failed":0}})", user_data);
    }
    return copyJson(R"({"completed":0,"failed":0})");
}

void swap_ffi_stop_maker_loop()
{
    makerLoopCancel.store(true);
}

void swap_ffi_free_string(char* ptr)
{
    std::free(ptr);
}

}
