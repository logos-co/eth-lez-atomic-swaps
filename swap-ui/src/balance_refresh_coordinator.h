#pragma once

#include <string>

// Where an automatic balance refresh can be served from. The two routes read
// the same two chains but take their credentials from different places:
//   env    — a .env file the user loaded (SwapUiPlugin::fetchBalancesFromLoadedEnv)
//   config — the fields the UI itself holds (SwapUiPlugin::fetchBalances)
// Anyone who configured through the guided Setup has only the second, which is
// everyone in practice; see route() for why that distinction is load-bearing.
struct BalanceRefreshSources
{
    bool env = false;
    bool config = false;

    bool any() const { return env || config; }
};

// The pair of numbers the account strip shows. Compared side-by-side (never as
// one joined string) so a settle-poll can tell "the ETH leg landed" from "both
// legs landed" — see observeSettle().
struct BalanceSnapshot
{
    std::string eth;
    std::string lez;
};

class BalanceRefreshCoordinator
{
public:
    enum class Decision {
        StartFromEnv,
        StartFromConfig,
        Coalesced,
        Unavailable,
    };

    // Which route serves a refresh right now. A loaded env file wins because it
    // is an explicit user choice; otherwise the config-backed route carries it.
    //
    // This used to be "env or nothing", which meant every automatic refresh in
    // the plugin — the post-swap settle poll included — took the Unavailable
    // branch for Setup-configured users and no-opped with a trace line.
    static Decision route(BalanceRefreshSources sources)
    {
        if (sources.env) {
            return Decision::StartFromEnv;
        }
        if (sources.config) {
            return Decision::StartFromConfig;
        }
        return Decision::Unavailable;
    }

    Decision requestAutomatic(BalanceRefreshSources sources, bool refreshInFlight)
    {
        if (!sources.any()) {
            return Decision::Unavailable;
        }
        if (refreshInFlight) {
            m_pendingAutomatic = true;
            return Decision::Coalesced;
        }
        return route(sources);
    }

    // Called when a refresh result lands: reports the route for the follow-up
    // refresh a coalesced request earned, or Unavailable when there is none.
    Decision finish(BalanceRefreshSources sources)
    {
        if (!m_pendingAutomatic) {
            return Decision::Unavailable;
        }
        m_pendingAutomatic = false;
        return route(sources);
    }

    // Settle-poll: when a swap completes, the freshly-claimed balance may not
    // be on-chain yet. The taker's LEZ claim is the sharp case —
    // LezClient::claim() returns as soon as the action is SUBMITTED, so
    // run_taker reports Completed up to a LEZ block (a minute or more on the
    // public testnet) before the credit exists, while the ETH leg confirmed
    // near the start of the run and is already visible. A refresh at completion
    // therefore reads the new ETH and the stale LEZ.
    //
    // beginSettle() snapshots both sides; observeSettle() is called with each
    // subsequent refresh result and returns true while the caller should keep
    // scheduling delayed refreshes — until BOTH sides have moved, or the window
    // closes (so a swap that legitimately moves nothing can't poll forever).
    //
    // Per-side is the whole point. A single joined "eth|lez" key called the
    // settle finished the instant the ETH leg moved, which is exactly the leg
    // that had already landed — leaving the received LEZ stale until restart.
    void beginSettle(const BalanceSnapshot& snapshot, long long nowMs, long long windowMs)
    {
        m_settleSnapshot = snapshot;
        m_settleDeadlineMs = nowMs + windowMs;
        m_ethMoved = false;
        m_lezMoved = false;
        m_settling = windowMs > 0;
    }

    bool observeSettle(const BalanceSnapshot& current, long long nowMs)
    {
        if (!m_settling) {
            return false;
        }
        // A failed read leaves the properties untouched, so an unchanged value
        // reads as "not landed yet" and simply earns another poll.
        if (current.eth != m_settleSnapshot.eth) {
            m_ethMoved = true;
        }
        if (current.lez != m_settleSnapshot.lez) {
            m_lezMoved = true;
        }
        if (m_ethMoved && m_lezMoved) {
            m_settling = false; // both legs landed — stop.
            return false;
        }
        if (nowMs >= m_settleDeadlineMs) {
            m_settling = false; // gave the chains long enough — stop.
            return false;
        }
        return true; // a side is still stale — poll again after the caller's delay.
    }

    bool isSettling() const { return m_settling; }

    void cancelSettle()
    {
        m_settling = false;
        m_settleDeadlineMs = 0;
    }

private:
    bool m_pendingAutomatic = false;
    bool m_settling = false;
    bool m_ethMoved = false;
    bool m_lezMoved = false;
    long long m_settleDeadlineMs = 0;
    BalanceSnapshot m_settleSnapshot;
};
