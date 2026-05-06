#include "../../lib/swap_ffi.h"

#include <cstdlib>
#include <cstring>

namespace {

char* copyJson(const char* json)
{
    const auto len = std::strlen(json);
    auto* out = static_cast<char*>(std::malloc(len + 1));
    std::memcpy(out, json, len + 1);
    return out;
}

} // namespace

extern "C" {

char* swap_ffi_load_env(const char*)
{
    return copyJson(R"({"ok":true,"method":"loadEnv"})");
}

char* swap_ffi_run_maker(const char*, const char*, ProgressCallback cb, void* user_data)
{
    if (cb) {
        cb(R"({"step":"maker-started"})", user_data);
    }
    return copyJson(R"({"ok":true,"method":"runMaker"})");
}

char* swap_ffi_run_taker(const char*, const char*, ProgressCallback cb, void* user_data)
{
    if (cb) {
        cb(R"({"step":"taker-started"})", user_data);
    }
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
    if (cb) {
        cb(R"({"step":"maker-loop-started"})", user_data);
    }
    return copyJson(R"({"ok":true,"method":"runMakerLoop"})");
}

void swap_ffi_stop_maker_loop() {}

void swap_ffi_free_string(char* ptr)
{
    std::free(ptr);
}

}
