#include "swap_impl.h"

#include <cstring>

SwapImpl::SwapImpl() = default;
SwapImpl::~SwapImpl() = default;

std::string SwapImpl::takeAndFree(char* ptr) {
    if (!ptr) {
        return std::string{};
    }
    std::string out{ptr};
    swap_ffi_free_string(ptr);
    return out;
}

void SwapImpl::progressTrampoline(const char* json, void* userData) {
    auto* ctx = static_cast<ProgressCtx*>(userData);
    if (!ctx || !ctx->self) {
        return;
    }
    if (ctx->self->emitEvent) {
        ctx->self->emitEvent(ctx->eventName, json ? std::string{json} : std::string{});
    }
}

std::string SwapImpl::loadEnv(const std::string& path) {
    return takeAndFree(swap_ffi_load_env(path.c_str()));
}

std::string SwapImpl::fetchBalances(const std::string& configJson) {
    return takeAndFree(swap_ffi_fetch_balances(configJson.c_str()));
}

std::string SwapImpl::messagingInit(const std::string& configJson) {
    return takeAndFree(swap_ffi_messaging_init(configJson.c_str()));
}

std::string SwapImpl::messagingShutdown() {
    return takeAndFree(swap_ffi_messaging_shutdown());
}

std::string SwapImpl::messagingStatus() {
    return takeAndFree(swap_ffi_messaging_status());
}

std::string SwapImpl::publishOffer(const std::string& configJson) {
    return takeAndFree(swap_ffi_publish_offer(configJson.c_str()));
}

std::string SwapImpl::fetchOffers() {
    return takeAndFree(swap_ffi_fetch_offers());
}

std::string SwapImpl::refundLez(const std::string& configJson, const std::string& hashlockHex) {
    return takeAndFree(swap_ffi_refund_lez(configJson.c_str(), hashlockHex.c_str()));
}

std::string SwapImpl::refundEth(const std::string& configJson, const std::string& swapIdHex) {
    return takeAndFree(swap_ffi_refund_eth(configJson.c_str(), swapIdHex.c_str()));
}

std::string SwapImpl::runMaker(const std::string& configJson, const std::string& hashlockHex) {
    ProgressCtx ctx{this, "maker.progress"};
    return takeAndFree(swap_ffi_run_maker(
        configJson.c_str(),
        hashlockHex.c_str(),
        &SwapImpl::progressTrampoline,
        &ctx));
}

std::string SwapImpl::runTaker(const std::string& configJson, const std::string& preimageHex) {
    ProgressCtx ctx{this, "taker.progress"};
    return takeAndFree(swap_ffi_run_taker(
        configJson.c_str(),
        preimageHex.c_str(),
        &SwapImpl::progressTrampoline,
        &ctx));
}

std::string SwapImpl::runMakerLoop(const std::string& configJson) {
    ProgressCtx ctx{this, "maker_loop.progress"};
    return takeAndFree(swap_ffi_run_maker_loop(
        configJson.c_str(),
        &SwapImpl::progressTrampoline,
        &ctx));
}

void SwapImpl::stopMakerLoop() {
    swap_ffi_stop_maker_loop();
}
