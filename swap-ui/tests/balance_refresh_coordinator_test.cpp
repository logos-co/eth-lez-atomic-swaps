#include "balance_refresh_coordinator.h"

#include <cassert>

int main()
{
    using Decision = BalanceRefreshCoordinator::Decision;

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(true, false) == Decision::Start);
        assert(!coordinator.finish(true));
    }

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(true, true) == Decision::Coalesced);
        assert(coordinator.finish(true));
        assert(!coordinator.finish(true));
    }

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(true, true) == Decision::Coalesced);
        assert(coordinator.requestAutomatic(true, true) == Decision::Coalesced);
        assert(coordinator.finish(true));
        assert(!coordinator.finish(true));
    }

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(false, false) == Decision::Unavailable);
        assert(!coordinator.finish(false));
    }

    {
        BalanceRefreshCoordinator coordinator;
        assert(coordinator.requestAutomatic(true, true) == Decision::Coalesced);
        assert(!coordinator.finish(false));
        assert(!coordinator.finish(true));
    }

    // --- settle-poll ---

    // Balance still stale → keep polling; changes on the 3rd look → stop.
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle("0.005|170", 8);
        assert(coordinator.isSettling());
        assert(coordinator.observeSettle("0.005|170"));  // unchanged → retry
        assert(coordinator.observeSettle("0.005|170"));  // unchanged → retry
        assert(!coordinator.observeSettle("0.0049|175")); // changed → stop
        assert(!coordinator.isSettling());
        // Once settled, no further polling.
        assert(!coordinator.observeSettle("0.0049|175"));
    }

    // Balance never changes (genuine no-op) → stop at the attempt cap, not forever.
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle("0.005|170", 3);
        assert(coordinator.observeSettle("0.005|170"));  // attempt 1 → retry
        assert(coordinator.observeSettle("0.005|170"));  // attempt 2 → retry
        assert(!coordinator.observeSettle("0.005|170")); // attempt 3 → cap hit, stop
        assert(!coordinator.isSettling());
    }

    // observeSettle without an active settle is a no-op (no stray reschedules).
    {
        BalanceRefreshCoordinator coordinator;
        assert(!coordinator.observeSettle("0.005|170"));
    }

    // cancelSettle() ends polling immediately (e.g. a new swap starts).
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle("0.005|170", 8);
        assert(coordinator.isSettling());
        coordinator.cancelSettle();
        assert(!coordinator.isSettling());
        assert(!coordinator.observeSettle("0.005|170"));
    }

    // A zero-attempt settle is inert.
    {
        BalanceRefreshCoordinator coordinator;
        coordinator.beginSettle("0.005|170", 0);
        assert(!coordinator.isSettling());
        assert(!coordinator.observeSettle("0.005|170"));
    }
}
