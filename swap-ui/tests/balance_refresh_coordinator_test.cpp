#include "balance_refresh_coordinator.h"

#include <cassert>

namespace {

BalanceRefreshSources envOnly()
{
    BalanceRefreshSources sources;
    sources.env = true;
    return sources;
}

// What every user who configured through the guided Setup has: no loaded .env
// file, a config the balance-read gate is happy with.
BalanceRefreshSources configOnly()
{
    BalanceRefreshSources sources;
    sources.config = true;
    return sources;
}

BalanceRefreshSources noSources()
{
    return BalanceRefreshSources{};
}

BalanceSnapshot snap(const char* eth, const char* lez)
{
    return BalanceSnapshot{eth, lez};
}

} // namespace

int main()
{
    using Decision = BalanceRefreshCoordinator::Decision;

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(envOnly(), false) == Decision::StartFromEnv);
        assert(coordinator.finish(envOnly()) == Decision::Unavailable);
    }

    // A config-backed user gets a real refresh, not a no-op. This is the whole
    // reason the post-swap settle poll reached nobody: routing used to be
    // "env or nothing", so every automatic refresh for a Setup-configured user
    // took the Unavailable branch and only wrote a trace line.
    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(configOnly(), false) == Decision::StartFromConfig);
    }

    // A loaded env file wins when both are available (an explicit user choice).
    {
        BalanceRefreshSources both;
        both.env = true;
        both.config = true;
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(both, false) == Decision::StartFromEnv);
    }

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(envOnly(), true) == Decision::Coalesced);
        assert(coordinator.finish(envOnly()) == Decision::StartFromEnv);
        assert(coordinator.finish(envOnly()) == Decision::Unavailable);
    }

    // The coalesced follow-up takes the config route when that is the source.
    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(configOnly(), true) == Decision::Coalesced);
        assert(coordinator.finish(configOnly()) == Decision::StartFromConfig);
        assert(coordinator.finish(configOnly()) == Decision::Unavailable);
    }

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(envOnly(), true) == Decision::Coalesced);
        assert(coordinator.requestAutomatic(envOnly(), true) == Decision::Coalesced);
        assert(coordinator.finish(envOnly()) == Decision::StartFromEnv);
        assert(coordinator.finish(envOnly()) == Decision::Unavailable);
    }

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(noSources(), false) == Decision::Unavailable);
        assert(coordinator.finish(noSources()) == Decision::Unavailable);
    }

    // The source went away while the refresh was in flight.
    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(envOnly(), true) == Decision::Coalesced);
        assert(coordinator.finish(noSources()) == Decision::Unavailable);
        assert(coordinator.finish(envOnly()) == Decision::Unavailable);
    }

    // --- settle-poll ---

    // THE regression this file exists for: the taker completes a swap, the ETH
    // leg (locked and confirmed near the start of the run) is already visible
    // at completion, and the LEZ claim it just SUBMITTED has not committed yet.
    // A settle that stops as soon as "the balance" changes calls it done on the
    // first read and leaves the received LEZ stale until the app is restarted.
    {
        const long long window = 300000;
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle(snap("0.0500", "170"), 0, window);
        assert(coordinator.isSettling());

        // t+4s: ETH moved (the lock), LEZ has not. MUST keep polling.
        assert(coordinator.observeSettle(snap("0.0480", "170"), 4000));
        // t+24s: where the old 6-tick window gave up. Still stale, still polling.
        assert(coordinator.observeSettle(snap("0.0480", "170"), 24000));
        // t+60s: a LEZ block goes by and the claim lands.
        assert(!coordinator.observeSettle(snap("0.0480", "175"), 60000));
        assert(!coordinator.isSettling());
        // Once settled, no further polling.
        assert(!coordinator.observeSettle(snap("0.0480", "175"), 75000));
    }

    // The mirror case (maker: the LEZ leg moves first, the ETH claim lands
    // later) is just as one-sided, and just as much a stop-too-early bug.
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle(snap("0.0500", "170"), 0, 300000);
        assert(coordinator.observeSettle(snap("0.0500", "165"), 5000));
        assert(!coordinator.observeSettle(snap("0.0520", "165"), 65000));
    }

    // A read that fails leaves both properties untouched; that is "not landed
    // yet", not "settled".
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle(snap("0.0500", "170"), 0, 300000);
        assert(coordinator.observeSettle(snap("0.0500", "170"), 15000));
        assert(coordinator.isSettling());
    }

    // Neither side ever moves (a swap that failed before either leg) → stop at
    // the window, not forever.
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle(snap("0.0500", "170"), 1000, 60000);
        assert(coordinator.observeSettle(snap("0.0500", "170"), 30000));
        assert(coordinator.observeSettle(snap("0.0500", "170"), 60000));
        assert(!coordinator.observeSettle(snap("0.0500", "170"), 61000));
        assert(!coordinator.isSettling());
    }

    // One side landed but the other never does → still bounded by the window.
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle(snap("0.0500", "170"), 0, 60000);
        assert(coordinator.observeSettle(snap("0.0480", "170"), 20000));
        assert(!coordinator.observeSettle(snap("0.0480", "170"), 60000));
        assert(!coordinator.isSettling());
    }

    // A side that moved stays moved, even though later reads compare against
    // the original snapshot.
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle(snap("0.0500", "170"), 0, 300000);
        assert(coordinator.observeSettle(snap("0.0480", "170"), 4000));
        // A stale/cached ETH read comes back with the pre-swap figure...
        assert(coordinator.observeSettle(snap("0.0500", "170"), 8000));
        // ...and the LEZ claim landing is still enough to finish the settle.
        assert(!coordinator.observeSettle(snap("0.0500", "175"), 60000));
    }

    // observeSettle without an active settle is a no-op (no stray reschedules).
    {
        BalanceRefreshCoordinator coordinator;
        assert(!coordinator.observeSettle(snap("0.0500", "170"), 1000));
    }

    // cancelSettle() ends polling immediately (e.g. a new swap starts).
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle(snap("0.0500", "170"), 0, 300000);
        assert(coordinator.isSettling());
        coordinator.cancelSettle();
        assert(!coordinator.isSettling());
        assert(!coordinator.observeSettle(snap("0.0500", "170"), 1000));
    }

    // A zero-length settle window is inert.
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle(snap("0.0500", "170"), 0, 0);
        assert(!coordinator.isSettling());
        assert(!coordinator.observeSettle(snap("0.0500", "170"), 0));
    }
}
