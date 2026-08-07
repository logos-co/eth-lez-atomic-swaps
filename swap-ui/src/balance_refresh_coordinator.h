#pragma once

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

private:
    bool m_pendingAutomatic = false;
};
