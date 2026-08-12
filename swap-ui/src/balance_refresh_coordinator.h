#pragma once

#include <string>

class BalanceRefreshCoordinator
{
public:
    enum class Decision {
        Start,
        Coalesced,
        Unavailable,
    };

    Decision requestAutomatic(bool sourceAvailable, bool refreshInFlight)
    {
        if (!sourceAvailable) {
            return Decision::Unavailable;
        }
        if (refreshInFlight) {
            m_pendingAutomatic = true;
            return Decision::Coalesced;
        }
        return Decision::Start;
    }

    bool finish(bool sourceAvailable)
    {
        if (!m_pendingAutomatic) {
            return false;
        }
        m_pendingAutomatic = false;
        return sourceAvailable;
    }

    // Settle-poll: when a swap completes, the freshly-claimed balance may not
    // be on-chain yet — a LEZ claim confirms ~1 min AFTER the swap "finishes".
    // A single refresh at completion reads the stale pre-claim balance, so the
    // header looks frozen until a manual Refresh. beginSettle() snapshots the
    // balance at completion; observeSettle() is called with each subsequent
    // refresh result and returns true while the caller should keep scheduling
    // delayed refreshes — until the balance actually changes, or a bounded
    // attempt cap is reached (so a genuinely no-op swap can't poll forever).
    void beginSettle(const std::string& snapshot, int maxAttempts)
    {
        m_settleSnapshot = snapshot;
        m_settleAttemptsLeft = maxAttempts;
        m_settling = maxAttempts > 0;
    }

    bool observeSettle(const std::string& currentBalance)
    {
        if (!m_settling) {
            return false;
        }
        if (currentBalance != m_settleSnapshot) {
            m_settling = false; // balance moved — the claim landed, stop.
            return false;
        }
        if (--m_settleAttemptsLeft <= 0) {
            m_settling = false; // gave the chain long enough — stop.
            return false;
        }
        return true; // still stale — poll again after the caller's delay.
    }

    bool isSettling() const { return m_settling; }

    void cancelSettle()
    {
        m_settling = false;
        m_settleAttemptsLeft = 0;
    }

private:
    bool m_pendingAutomatic = false;
    bool m_settling = false;
    int m_settleAttemptsLeft = 0;
    std::string m_settleSnapshot;
};
