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
}
