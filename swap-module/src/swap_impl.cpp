#include "swap_impl.h"

#include "swap_delivery_adapter.h"

#include <chrono>
#include <cstdio>
#include <cstring>
#include <cctype>
#include <sstream>
#include <thread>

#include <QByteArray>
#include <QDateTime>
#include <QDir>
#include <QFile>
#include <QMetaObject>
#include <QObject>
#include <QPointer>
#include <QString>
#include <QThread>

#ifdef emit
#undef emit
#endif

namespace {
// Out-of-band trace logger — Basecamp's Qt message handler swallows
// qInfo/qWarning, so we append to a per-instance file under
// XDG_RUNTIME_DIR (isolated to /tmp/lbc-{maker,taker} by
// scripts/basecamp-instance.sh) and to stderr with explicit fflush.
void swapImplTrace(const std::string& msg)
{
    const QString runtimeDir = qEnvironmentVariable("XDG_RUNTIME_DIR");
    const QString path = !runtimeDir.isEmpty()
        ? QDir(runtimeDir).filePath(QStringLiteral("swap-module.trace.log"))
        : QDir::temp().filePath(QStringLiteral("swap-module.trace.log"));

    const QByteArray line =
        QDateTime::currentDateTimeUtc().toString(Qt::ISODateWithMs).toUtf8()
        + " [swap_impl] " + QByteArray::fromStdString(msg) + "\n";

    QFile f(path);
    if (f.open(QIODevice::WriteOnly | QIODevice::Append | QIODevice::Text)) {
        f.write(line);
        f.flush();
    }

    std::fwrite(line.constData(), 1, static_cast<size_t>(line.size()), stderr);
    std::fflush(stderr);
}
} // namespace

struct SwapImpl::EmitterState {
    std::mutex mutex;
    bool active = true;
    std::function<void(const std::string& eventName, const std::string& data)> emit;
    QPointer<QObject> dispatcher;
};

struct SwapImpl::JobState {
    mutable std::mutex mutex;
    std::string id;
    std::string role;
    std::string status = "running";
    std::string step;
    std::string resultJson;
    std::string error;
    std::string lastProgressJson;
    bool cancelRequested = false;
};

namespace {

std::string jsonEscape(const std::string& raw)
{
    std::string out;
    out.reserve(raw.size() + 8);
    for (char c : raw) {
        switch (c) {
        case '\\': out += "\\\\"; break;
        case '"': out += "\\\""; break;
        case '\n': out += "\\n"; break;
        case '\r': out += "\\r"; break;
        case '\t': out += "\\t"; break;
        default:
            if (static_cast<unsigned char>(c) < 0x20) {
                out += "?";
            } else {
                out += c;
            }
        }
    }
    return out;
}

std::string jsonString(const std::string& raw)
{
    return "\"" + jsonEscape(raw) + "\"";
}

bool looksLikeJsonValue(const std::string& raw)
{
    const auto first = raw.find_first_not_of(" \t\r\n");
    if (first == std::string::npos) {
        return false;
    }
    const char c = raw[first];
    return c == '{' || c == '[' || c == '"' || c == '-' || std::isdigit(static_cast<unsigned char>(c))
        || raw.compare(first, 4, "true") == 0
        || raw.compare(first, 5, "false") == 0
        || raw.compare(first, 4, "null") == 0;
}

std::string jsonValueOrString(const std::string& raw)
{
    return looksLikeJsonValue(raw) ? raw : jsonString(raw);
}

std::string extractJsonString(const std::string& json, const std::string& key)
{
    const std::string needle = "\"" + key + "\"";
    auto pos = json.find(needle);
    if (pos == std::string::npos) {
        return {};
    }
    pos = json.find(':', pos + needle.size());
    if (pos == std::string::npos) {
        return {};
    }
    ++pos;
    while (pos < json.size() && std::isspace(static_cast<unsigned char>(json[pos]))) {
        ++pos;
    }
    if (pos >= json.size() || json[pos] != '"') {
        return {};
    }
    ++pos;
    std::string out;
    bool escaped = false;
    for (; pos < json.size(); ++pos) {
        const char c = json[pos];
        if (escaped) {
            switch (c) {
            case 'n': out += '\n'; break;
            case 'r': out += '\r'; break;
            case 't': out += '\t'; break;
            default: out += c; break;
            }
            escaped = false;
        } else if (c == '\\') {
            escaped = true;
        } else if (c == '"') {
            return out;
        } else {
            out += c;
        }
    }
    return {};
}

std::string extractJsonValue(const std::string& json, const std::string& key)
{
    const std::string needle = "\"" + key + "\"";
    auto pos = json.find(needle);
    if (pos == std::string::npos) {
        return {};
    }
    pos = json.find(':', pos + needle.size());
    if (pos == std::string::npos) {
        return {};
    }
    ++pos;
    while (pos < json.size() && std::isspace(static_cast<unsigned char>(json[pos]))) {
        ++pos;
    }
    if (pos >= json.size()) {
        return {};
    }

    const char first = json[pos];
    if (first == '{' || first == '[') {
        const char open = first;
        const char close = first == '{' ? '}' : ']';
        int depth = 0;
        bool inString = false;
        bool escaped = false;
        for (std::size_t i = pos; i < json.size(); ++i) {
            const char c = json[i];
            if (inString) {
                if (escaped) {
                    escaped = false;
                } else if (c == '\\') {
                    escaped = true;
                } else if (c == '"') {
                    inString = false;
                }
                continue;
            }
            if (c == '"') {
                inString = true;
            } else if (c == open) {
                ++depth;
            } else if (c == close) {
                --depth;
                if (depth == 0) {
                    return json.substr(pos, i - pos + 1);
                }
            }
        }
        return {};
    }

    if (first == '"') {
        std::size_t i = pos + 1;
        bool escaped = false;
        for (; i < json.size(); ++i) {
            const char c = json[i];
            if (escaped) {
                escaped = false;
            } else if (c == '\\') {
                escaped = true;
            } else if (c == '"') {
                return json.substr(pos, i - pos + 1);
            }
        }
        return {};
    }

    auto end = pos;
    while (end < json.size() && json[end] != ',' && json[end] != '}'
           && !std::isspace(static_cast<unsigned char>(json[end]))) {
        ++end;
    }
    return json.substr(pos, end - pos);
}

std::string resultError(const std::string& resultJson)
{
    return extractJsonString(resultJson, "error");
}

} // namespace

SwapImpl::SwapImpl()
    : m_emitter(std::make_shared<EmitterState>())
{
}

SwapImpl::~SwapImpl()
{
    QPointer<QObject> dispatcher;
    std::vector<std::thread> workers;
    {
        std::lock_guard<std::mutex> lock(m_emitter->mutex);
        m_emitter->active = false;
        m_emitter->emit = nullptr;
        dispatcher = m_emitter->dispatcher;
        m_emitter->dispatcher = nullptr;
    }

    {
        std::lock_guard<std::mutex> lock(m_jobsMutex);
        for (auto& entry : m_jobs) {
            std::lock_guard<std::mutex> jobLock(entry.second->mutex);
            if (!isTerminalStatus(entry.second->status)) {
                entry.second->cancelRequested = true;
                entry.second->status = "cancelling";
            }
        }
    }
    swap_ffi_stop_maker_loop();

    {
        std::lock_guard<std::mutex> lock(m_workersMutex);
        workers.swap(m_workers);
    }

    for (auto& worker : workers) {
        if (!worker.joinable()) {
            continue;
        }
        if (worker.get_id() == std::this_thread::get_id()) {
            worker.detach();
            continue;
        }
        worker.join();
    }

    if (dispatcher) {
        dispatcher->deleteLater();
    }
}

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
    if (!ctx || !ctx->job) {
        return;
    }
    const auto payload = progressPayload(ctx->job, json ? std::string{json} : std::string{});
    safeEmit(ctx->emitter, ctx->progressEventName, payload);
}

std::string SwapImpl::persistenceRoot() {
    // Read the host-stamped `instancePersistencePath` property off the module's
    // LogosAPI, captured by the generated provider's onInit and stashed in the
    // delivery adapter. Empty outside a persistence-provisioning host (unit
    // tests / lgpd); swap_ui treats an empty return as "unknown" and falls back
    // to LOGOS_USER_DIR / its legacy path.
    const std::string root = swapDeliveryRuntimePersistencePath();
    // Trace the resolved per-profile root so a release/verification grep can
    // confirm the module is handing swap_ui a profile-scoped path rather than
    // the shared ui-host tree (issue #99).
    swapImplTrace("swap persistence root (per-profile): " + root);
    return root;
}

std::string SwapImpl::loadEnv(const std::string& path) {
    return takeAndFree(swap_ffi_load_env(path.c_str()));
}

std::string SwapImpl::defaultLezHtlcProgramId() {
    return takeAndFree(swap_ffi_default_lez_htlc_program_id());
}

std::string SwapImpl::fetchBalances(const std::string& configJson) {
    return takeAndFree(swap_ffi_fetch_balances(configJson.c_str()));
}

std::string SwapImpl::fetchBalancesFromEnv(const std::string& path) {
    const auto configJson = takeAndFree(swap_ffi_load_env(path.c_str()));
    if (!resultError(configJson).empty()) {
        return configJson;
    }
    return takeAndFree(swap_ffi_fetch_balances(configJson.c_str()));
}

std::string SwapImpl::messagingInit(const std::string& configJson) {
    return swapDeliveryMessagingInit(configJson);
}

std::string SwapImpl::messagingShutdown() {
    return swapDeliveryMessagingShutdown();
}

std::string SwapImpl::messagingStatus() {
    return swapDeliveryMessagingStatus();
}

std::string SwapImpl::publishOffer(const std::string& configJson) {
    return swapDeliveryPublishOffer(configJson);
}

std::string SwapImpl::fetchOffers() {
    return swapDeliveryFetchOffers();
}

std::string SwapImpl::subscribeSwap(const std::string& hashlockHex) {
    return swapDeliverySubscribeSwap(hashlockHex);
}

std::string SwapImpl::unsubscribeSwap(const std::string& hashlockHex) {
    return swapDeliveryUnsubscribeSwap(hashlockHex);
}

std::string SwapImpl::publishSwapAccept(const std::string& configJson) {
    return swapDeliveryPublishSwapAccept(configJson);
}

std::string SwapImpl::fetchSwapEvents(const std::string& hashlockHex) {
    return swapDeliveryFetchSwapEvents(hashlockHex);
}

std::string SwapImpl::refundLez(const std::string& configJson, const std::string& hashlockHex) {
    return takeAndFree(swap_ffi_refund_lez(configJson.c_str(), hashlockHex.c_str()));
}

std::string SwapImpl::refundEth(const std::string& configJson, const std::string& swapIdHex) {
    return takeAndFree(swap_ffi_refund_eth(configJson.c_str(), swapIdHex.c_str()));
}

std::string SwapImpl::generateEthKey() {
    return takeAndFree(swap_ffi_generate_eth_key());
}

std::string SwapImpl::generateLezAccount() {
    return takeAndFree(swap_ffi_generate_lez_account());
}

std::string SwapImpl::lezEnsureInitialized(const std::string& sequencerUrl,
                                           const std::string& signingKeyHex) {
    return takeAndFree(swap_ffi_lez_ensure_initialized(sequencerUrl.c_str(), signingKeyHex.c_str()));
}

std::string SwapImpl::runMaker(const std::string& configJson, const std::string& hashlockHex) {
    return runBlockingJob("maker", configJson, hashlockHex);
}

std::string SwapImpl::runTaker(const std::string& configJson, const std::string& preimageHex) {
    return runBlockingJob("taker", configJson, preimageHex);
}

std::string SwapImpl::runMakerLoop(const std::string& configJson) {
    return runBlockingJob("maker_loop", configJson, {});
}

void SwapImpl::stopMakerLoop() {
    std::string activeJobId;
    {
        std::lock_guard<std::mutex> lock(m_jobsMutex);
        if (m_makerLoopJob) {
            std::lock_guard<std::mutex> jobLock(m_makerLoopJob->mutex);
            if (!isTerminalStatus(m_makerLoopJob->status)) {
                activeJobId = m_makerLoopJob->id;
            }
        }
    }
    if (!activeJobId.empty()) {
        (void)stopJob(activeJobId);
        return;
    }
    swap_ffi_stop_maker_loop();
}

std::string SwapImpl::startMakerJob(const std::string& configJson, const std::string& hashlockHex)
{
    return startJob("maker", configJson, hashlockHex);
}

std::string SwapImpl::startTakerJob(const std::string& configJson, const std::string& preimageHex)
{
    return startJob("taker", configJson, preimageHex);
}

std::string SwapImpl::startMakerLoopJob(const std::string& configJson)
{
    return startJob("maker_loop", configJson, {});
}

// 150 LEZ: exactly one native pinata claim (see src/lez/onboard.rs — 150
// LEZ/claim). A taker mostly RECEIVES LEZ over the lifetime of an account
// (it locks ETH and claims LEZ that a maker already funded into escrow), so
// onboarding needs the account to exist and be initialized far more than it
// needs a large standing balance — one claim is enough for it to be a
// legitimate on-chain counterparty and to cover any small transfer of its
// own. Each claim also costs real CPU time (PoW at difficulty 3) and a real
// chain round trip, so onboarding requests exactly one rather than looping
// to a bigger number "for headroom". Matches the Config tab's own default
// lez_amount ("150" — see SwapUiPlugin::applyDefaultConfig).
const char* const kDefaultSetupFundingTargetLez = "150";

std::string SwapImpl::startLezFundingJob(const std::string& sequencerUrl,
                                         const std::string& signingKeyHex,
                                         const std::string& targetLezArg)
{
    const std::string role = "lez_setup";
    const std::string targetLez = targetLezArg.empty() ? kDefaultSetupFundingTargetLez : targetLezArg;

    const bool emitterBoundBeforeAssign = static_cast<bool>(emitEvent);
    {
        std::lock_guard<std::mutex> lock(m_emitter->mutex);
        m_emitter->active = true;
        m_emitter->emit = emitEvent;
        if (!m_emitter->dispatcher) {
            m_emitter->dispatcher = new QObject();
        }
    }
    swapImplTrace(std::string("startLezFundingJob emitEvent_bound=")
                  + (emitterBoundBeforeAssign ? "true" : "false") + " target=" + targetLez);

    auto job = std::make_shared<JobState>();
    job->id = newJobId(role, m_nextJobId.fetch_add(1));
    job->role = role;

    {
        std::lock_guard<std::mutex> lock(m_jobsMutex);
        auto active = activeJobForRoleLocked(role);
        if (active) {
            std::lock_guard<std::mutex> jobLock(active->mutex);
            if (!isTerminalStatus(active->status)) {
                std::ostringstream out;
                out << "{\"ok\":false,\"error\":" << jsonString(role + " job already running")
                    << ",\"role\":" << jsonString(role)
                    << ",\"active_job_id\":" << jsonString(active->id)
                    << "}";
                return out.str();
            }
        }
        m_jobs[job->id] = job;
        setActiveJobForRoleLocked(role, job);
    }

    auto emitter = m_emitter;
    std::thread worker([job, emitter, role, sequencerUrl, signingKeyHex, targetLez]() {
        ProgressCtx ctx{job, emitter, progressEventName(role)};
        std::string result = takeAndFree(swap_ffi_lez_claim_to_target(
            sequencerUrl.c_str(),
            signingKeyHex.c_str(),
            targetLez.c_str(),
            &SwapImpl::progressTrampoline,
            &ctx));

        setJobFinished(job, result);
        safeEmit(emitter, finishedEventName(role), finishedPayload(job));
    });

    {
        std::lock_guard<std::mutex> lock(m_workersMutex);
        m_workers.emplace_back(std::move(worker));
    }

    return jobJson(job);
}

std::string SwapImpl::stopJob(const std::string& jobId)
{
    std::shared_ptr<JobState> job;
    {
        std::lock_guard<std::mutex> lock(m_jobsMutex);
        auto it = m_jobs.find(jobId);
        if (it == m_jobs.end()) {
            return errorJson("unknown job_id");
        }
        job = it->second;
    }

    bool stopMakerLoopJob = false;
    {
        std::lock_guard<std::mutex> lock(job->mutex);
        job->cancelRequested = true;
        stopMakerLoopJob = job->role == "maker_loop" && !isTerminalStatus(job->status);
        if (!isTerminalStatus(job->status)) {
            job->status = "cancelling";
        }
    }

    if (stopMakerLoopJob) {
        swap_ffi_stop_maker_loop();
    }
    return jobJson(job);
}

std::string SwapImpl::jobStatus(const std::string& jobId)
{
    std::lock_guard<std::mutex> lock(m_jobsMutex);
    const auto it = m_jobs.find(jobId);
    if (it == m_jobs.end()) {
        return errorJson("unknown job_id");
    }
    return jobJson(it->second);
}

std::string SwapImpl::startJob(const std::string& roleArg,
                               const std::string& configJson,
                               const std::string& secretHex)
{
    const auto role = normalizeRole(roleArg);
    if (role.empty()) {
        return errorJson("invalid job role");
    }

    const bool emitterBoundBeforeAssign = static_cast<bool>(emitEvent);
    {
        std::lock_guard<std::mutex> lock(m_emitter->mutex);
        m_emitter->active = true;
        m_emitter->emit = emitEvent;
        if (!m_emitter->dispatcher) {
            m_emitter->dispatcher = new QObject();
        }
    }
    swapImplTrace(std::string("startJob role=") + role
                  + " emitEvent_bound=" + (emitterBoundBeforeAssign ? "true" : "false"));

    auto job = std::make_shared<JobState>();
    job->id = newJobId(role, m_nextJobId.fetch_add(1));
    job->role = role;

    {
        std::lock_guard<std::mutex> lock(m_jobsMutex);
        auto active = activeJobForRoleLocked(role);
        if (active) {
            std::lock_guard<std::mutex> jobLock(active->mutex);
            if (!isTerminalStatus(active->status)) {
                std::ostringstream out;
                out << "{\"ok\":false,\"error\":" << jsonString(role + " job already running")
                    << ",\"role\":" << jsonString(role)
                    << ",\"active_job_id\":" << jsonString(active->id)
                    << "}";
                return out.str();
            }
        }
        m_jobs[job->id] = job;
        setActiveJobForRoleLocked(role, job);
    }

    auto emitter = m_emitter;
    std::thread worker([job, emitter, role, configJson, secretHex]() {
        ProgressCtx ctx{job, emitter, progressEventName(role)};
        std::string result;
        if (role == "maker") {
            result = takeAndFree(swap_ffi_run_maker(
                configJson.c_str(),
                secretHex.c_str(),
                &SwapImpl::progressTrampoline,
                &ctx));
        } else if (role == "taker") {
            result = takeAndFree(swap_ffi_run_taker(
                configJson.c_str(),
                secretHex.c_str(),
                &SwapImpl::progressTrampoline,
                &ctx));
        } else {
            result = takeAndFree(swap_ffi_run_maker_loop(
                configJson.c_str(),
                &SwapImpl::progressTrampoline,
                &ctx));
        }

        setJobFinished(job, result);
        safeEmit(emitter, finishedEventName(role), finishedPayload(job));
    });

    {
        std::lock_guard<std::mutex> lock(m_workersMutex);
        m_workers.emplace_back(std::move(worker));
    }

    return jobJson(job);
}

std::shared_ptr<SwapImpl::JobState> SwapImpl::activeJobForRoleLocked(const std::string& role) const
{
    if (role == "maker") {
        return m_makerJob;
    }
    if (role == "taker") {
        return m_takerJob;
    }
    if (role == "maker_loop") {
        return m_makerLoopJob;
    }
    if (role == "lez_setup") {
        return m_lezSetupJob;
    }
    return {};
}

void SwapImpl::setActiveJobForRoleLocked(const std::string& role, const std::shared_ptr<JobState>& job)
{
    if (role == "maker") {
        m_makerJob = job;
    } else if (role == "taker") {
        m_takerJob = job;
    } else if (role == "maker_loop") {
        m_makerLoopJob = job;
    } else if (role == "lez_setup") {
        m_lezSetupJob = job;
    }
}

std::string SwapImpl::runBlockingJob(const std::string& roleArg,
                                     const std::string& configJson,
                                     const std::string& secretHex)
{
    const auto role = normalizeRole(roleArg);
    auto job = std::make_shared<JobState>();
    job->id = newJobId("sync_" + role, m_nextJobId.fetch_add(1));
    job->role = role;

    {
        std::lock_guard<std::mutex> lock(m_emitter->mutex);
        m_emitter->active = true;
        m_emitter->emit = emitEvent;
        if (!m_emitter->dispatcher) {
            m_emitter->dispatcher = new QObject();
        }
    }

    ProgressCtx ctx{job, m_emitter, progressEventName(role)};
    std::string result;
    if (role == "maker") {
        result = takeAndFree(swap_ffi_run_maker(
            configJson.c_str(),
            secretHex.c_str(),
            &SwapImpl::progressTrampoline,
            &ctx));
    } else if (role == "taker") {
        result = takeAndFree(swap_ffi_run_taker(
            configJson.c_str(),
            secretHex.c_str(),
            &SwapImpl::progressTrampoline,
            &ctx));
    } else {
        result = takeAndFree(swap_ffi_run_maker_loop(
            configJson.c_str(),
            &SwapImpl::progressTrampoline,
            &ctx));
    }

    setJobFinished(job, result);
    safeEmit(m_emitter, finishedEventName(role), finishedPayload(job));
    return result;
}

std::string SwapImpl::newJobId(const std::string& role, uint64_t id)
{
    return role + "-" + std::to_string(id);
}

std::string SwapImpl::progressEventName(const std::string& role)
{
    return role + ".progress";
}

std::string SwapImpl::finishedEventName(const std::string& role)
{
    return role + ".finished";
}

std::string SwapImpl::normalizeRole(const std::string& role)
{
    if (role == "maker" || role == "taker" || role == "maker_loop") {
        return role;
    }
    return {};
}

bool SwapImpl::isTerminalStatus(const std::string& status)
{
    return status == "completed" || status == "failed" || status == "cancelled";
}

int64_t SwapImpl::timestampMs()
{
    const auto now = std::chrono::system_clock::now().time_since_epoch();
    return std::chrono::duration_cast<std::chrono::milliseconds>(now).count();
}

void SwapImpl::safeEmit(const std::shared_ptr<EmitterState>& emitter,
                        const std::string& eventName,
                        const std::string& payload)
{
    if (!emitter) {
        swapImplTrace("safeEmit DROP eventName=" + eventName + " reason=null_emitter_state");
        return;
    }

    auto invoke = [emitter, eventName, payload]() {
        std::function<void(const std::string&, const std::string&)> emit;
        bool active = false;
        bool emitSet = false;
        {
            std::lock_guard<std::mutex> lock(emitter->mutex);
            active = emitter->active;
            emitSet = static_cast<bool>(emitter->emit);
            if (!active || !emitSet) {
                emit = nullptr;
            } else {
                emit = emitter->emit;
            }
        }
        if (!emit) {
            swapImplTrace("safeEmit DROP eventName=" + eventName
                          + " reason=invoke_no_callback active=" + (active ? "true" : "false")
                          + " emitSet=" + (emitSet ? "true" : "false"));
            return;
        }
        swapImplTrace("safeEmit FIRING eventName=" + eventName
                      + " payload.bytes=" + std::to_string(payload.size()));
        emit(eventName, payload);
    };

    QPointer<QObject> dispatcher;
    {
        std::lock_guard<std::mutex> lock(emitter->mutex);
        dispatcher = emitter->dispatcher;
        if (!dispatcher) {
            swapImplTrace("safeEmit DROP eventName=" + eventName + " reason=no_dispatcher");
            return;
        }
        if (QThread::currentThread() != dispatcher->thread()) {
            swapImplTrace("safeEmit QUEUEING eventName=" + eventName
                          + " (off-thread; payload.bytes=" + std::to_string(payload.size()) + ")");
            QMetaObject::invokeMethod(dispatcher.data(), std::move(invoke), Qt::QueuedConnection);
            return;
        }
    }

    swapImplTrace("safeEmit DIRECT eventName=" + eventName
                  + " payload.bytes=" + std::to_string(payload.size())
                  + " dispatcher=" + (dispatcher ? "yes" : "no"));
    invoke();
}

std::string SwapImpl::progressPayload(const std::shared_ptr<JobState>& job,
                                      const std::string& rawProgressJson)
{
    std::string step = extractJsonString(rawProgressJson, "step");
    std::string data = extractJsonValue(rawProgressJson, "data");
    if (data.empty()) {
        data = !step.empty()
            ? "{}"
            : (rawProgressJson.empty()
            ? "{}"
            : "{\"raw\":" + jsonValueOrString(rawProgressJson) + "}");
    }
    if (step.empty()) {
        step = rawProgressJson.empty() ? "progress" : rawProgressJson;
    }

    std::string id;
    std::string role;
    {
        std::lock_guard<std::mutex> lock(job->mutex);
        job->step = step;
        job->lastProgressJson = rawProgressJson;
        id = job->id;
        role = job->role;
    }

    std::ostringstream out;
    out << "{\"job_id\":" << jsonString(id)
        << ",\"role\":" << jsonString(role)
        << ",\"step\":" << jsonString(step)
        << ",\"data\":" << data
        << ",\"result\":null"
        << ",\"error\":null"
        << ",\"timestamp_ms\":" << timestampMs()
        << "}";
    return out.str();
}

std::string SwapImpl::finishedPayload(const std::shared_ptr<JobState>& job)
{
    std::string id;
    std::string role;
    std::string status;
    std::string step;
    std::string result;
    std::string error;
    {
        std::lock_guard<std::mutex> lock(job->mutex);
        id = job->id;
        role = job->role;
        status = job->status;
        step = job->step.empty() ? status : job->step;
        result = job->resultJson;
        error = job->error;
    }

    std::ostringstream out;
    out << "{\"job_id\":" << jsonString(id)
        << ",\"role\":" << jsonString(role)
        << ",\"step\":" << jsonString(step)
        << ",\"data\":{}"
        << ",\"result\":" << (result.empty() ? "null" : jsonValueOrString(result))
        << ",\"error\":" << (error.empty() ? "null" : jsonString(error))
        << ",\"timestamp_ms\":" << timestampMs()
        << "}";
    return out.str();
}

std::string SwapImpl::jobJson(const std::shared_ptr<JobState>& job)
{
    std::string id;
    std::string role;
    std::string status;
    std::string step;
    std::string result;
    std::string error;
    bool cancelRequested = false;
    {
        std::lock_guard<std::mutex> lock(job->mutex);
        id = job->id;
        role = job->role;
        status = job->status;
        step = job->step;
        result = job->resultJson;
        error = job->error;
        cancelRequested = job->cancelRequested;
    }

    std::ostringstream out;
    out << "{\"ok\":true"
        << ",\"job_id\":" << jsonString(id)
        << ",\"role\":" << jsonString(role)
        << ",\"status\":" << jsonString(status)
        << ",\"step\":" << jsonString(step)
        << ",\"result\":" << (result.empty() ? "null" : jsonValueOrString(result))
        << ",\"error\":" << (error.empty() ? "null" : jsonString(error))
        << ",\"cancel_requested\":" << (cancelRequested ? "true" : "false")
        << ",\"timestamp_ms\":" << timestampMs()
        << "}";
    return out.str();
}

std::string SwapImpl::errorJson(const std::string& error)
{
    return "{\"ok\":false,\"error\":" + jsonString(error) + "}";
}

void SwapImpl::setJobFinished(const std::shared_ptr<JobState>& job, const std::string& resultJson)
{
    const auto error = resultError(resultJson);
    std::lock_guard<std::mutex> lock(job->mutex);
    job->resultJson = resultJson;
    if (!error.empty()) {
        job->status = "failed";
        job->error = error;
    } else if (job->cancelRequested && job->role != "maker_loop") {
        job->status = "cancelled";
        job->error = "cancelled";
    } else {
        job->status = "completed";
        job->error.clear();
    }
}
