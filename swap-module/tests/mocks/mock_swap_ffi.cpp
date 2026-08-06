#include "../../lib/swap_ffi.h"

#include <cstdlib>
#include <cstring>
#include <atomic>
#include <chrono>
#include <thread>
#include <mutex>
#include <string>

namespace {

char* copyJson(const char* json)
{
    const auto len = std::strlen(json);
    auto* out = static_cast<char*>(std::malloc(len + 1));
    std::memcpy(out, json, len + 1);
    return out;
}

std::atomic_bool makerLoopCancel{false};
std::atomic_int loadEnvCalls{0};
std::atomic_int fetchBalancesCalls{0};
std::atomic_int freeStringCalls{0};
std::atomic_bool loadEnvShouldError{false};
std::mutex mockStateMutex;
std::string lastLoadEnvPath;
std::string lastFetchBalancesConfig;
std::string callSequence;

} // namespace

extern "C" {

char* swap_ffi_load_env(const char* path)
{
    loadEnvCalls.fetch_add(1);
    {
        std::lock_guard<std::mutex> lock(mockStateMutex);
        lastLoadEnvPath = path ? path : "";
        callSequence += callSequence.empty() ? "loadEnv" : ">loadEnv";
    }
    if (loadEnvShouldError.load()) {
        return copyJson(R"({"error":"forced load env failure"})");
    }
    return copyJson(R"({"eth_rpc_url":"ws://127.0.0.1:8545","eth_private_key":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","eth_htlc_address":"0x1111111111111111111111111111111111111111","lez_sequencer_url":"http://localhost:8080","lez_signing_key":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","lez_htlc_program_id":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","lez_amount":"1","eth_amount":"1","eth_recipient_address":"0x2222222222222222222222222222222222222222","lez_taker_account_id":"11111111111111111111111111111111","eth_timelock_minutes":"10","lez_timelock_minutes":"5","poll_interval_ms":"2000"})");
}

char* swap_ffi_run_maker(const char*, const char*, ProgressCallback cb, void* user_data)
{
    if (cb) {
        cb(R"({"step":"WaitingForEthLock"})", user_data);
        cb(R"({"step":"EthLockDetected","data":{"swap_id":"0xabc","hashlock":"abababababababababababababababababababababababababababababababab","taker_lez_account":"7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e","tx_hash":"0xdeadbeef00000000000000000000000000000000000000000000000000dead","chain_id":31337}})", user_data);
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    return copyJson(R"({"ok":true,"method":"runMaker"})");
}

char* swap_ffi_run_taker(const char*, const char*, ProgressCallback cb, void* user_data)
{
    if (cb) {
        cb(R"({"step":"PreimageGenerated","data":{"hashlock":"abc"}})", user_data);
        cb(R"({"step":"EthLocked","data":{"swap_id":"0xdef","tx_hash":"0xbeefdead00000000000000000000000000000000000000000000000000beef","chain_id":31337}})", user_data);
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    return copyJson(R"({"ok":true,"method":"runTaker"})");
}

char* swap_ffi_refund_lez(const char*, const char*)
{
    return copyJson(R"({"ok":true,"method":"refundLez"})");
}

char* swap_ffi_refund_eth(const char*, const char*)
{
    return copyJson(R"({"ok":true,"method":"refundEth"})");
}

char* swap_ffi_default_lez_htlc_program_id()
{
    return copyJson("9eb88f51aae87a58fb74b8d2dc7327b39333585e63280e3f9cf8d86dac0ed702");
}

char* swap_ffi_generate_eth_key()
{
    return copyJson(R"({"private_key":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","address":"0x1111111111111111111111111111111111111111"})");
}

char* swap_ffi_generate_lez_account()
{
    return copyJson(R"({"signing_key":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","account_id":"11111111111111111111111111111111"})");
}

char* swap_ffi_lez_ensure_initialized(const char*, const char*)
{
    return copyJson(R"({"outcome":"Initialized","data":{"tx_hash":"0xmockinit"}})");
}

char* swap_ffi_lez_claim_to_target(const char*, const char*, const char*, ProgressCallback cb, void* user_data)
{
    // Mirrors src/lez/onboard.rs::FundingProgress serialization exactly:
    // #[serde(tag = "step", content = "data")], and Claimed only fires once
    // the claim COMMITTED (so it carries the post-commit balance).
    if (cb) {
        cb(R"({"step":"Initializing"})", user_data);
        cb(R"({"step":"CheckingBalance","data":{"balance":0,"target":150}})", user_data);
        cb(R"({"step":"ClaimSubmitted","data":{"tx_hash":"0xmockclaim"}})", user_data);
        cb(R"({"step":"Claimed","data":{"tx_hash":"0xmockclaim","total_claims":1,"balance":150}})", user_data);
        cb(R"({"step":"TargetReached","data":{"balance":150}})", user_data);
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    return copyJson(R"({"balance":"150"})");
}

char* swap_ffi_fetch_balances(const char* config_json)
{
    fetchBalancesCalls.fetch_add(1);
    {
        std::lock_guard<std::mutex> lock(mockStateMutex);
        lastFetchBalancesConfig = config_json ? config_json : "";
        callSequence += callSequence.empty() ? "fetchBalances" : ">fetchBalances";
    }
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
    freeStringCalls.fetch_add(1);
    std::free(ptr);
}

void mock_swap_ffi_reset()
{
    loadEnvCalls.store(0);
    fetchBalancesCalls.store(0);
    freeStringCalls.store(0);
    loadEnvShouldError.store(false);
    std::lock_guard<std::mutex> lock(mockStateMutex);
    lastLoadEnvPath.clear();
    lastFetchBalancesConfig.clear();
    callSequence.clear();
}

void mock_swap_ffi_set_load_env_error(bool enabled)
{
    loadEnvShouldError.store(enabled);
}

int mock_swap_ffi_load_env_calls()
{
    return loadEnvCalls.load();
}

int mock_swap_ffi_fetch_balances_calls()
{
    return fetchBalancesCalls.load();
}

int mock_swap_ffi_free_string_calls()
{
    return freeStringCalls.load();
}

const char* mock_swap_ffi_last_load_env_path()
{
    std::lock_guard<std::mutex> lock(mockStateMutex);
    return lastLoadEnvPath.c_str();
}

const char* mock_swap_ffi_last_fetch_balances_config()
{
    std::lock_guard<std::mutex> lock(mockStateMutex);
    return lastFetchBalancesConfig.c_str();
}

const char* mock_swap_ffi_call_sequence()
{
    std::lock_guard<std::mutex> lock(mockStateMutex);
    return callSequence.c_str();
}

}
