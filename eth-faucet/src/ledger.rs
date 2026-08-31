//! Who got what, when — and whether the next claim is allowed.
//!
//! Deliberately a pure value: no clock, no network, no async. Every method
//! takes `now` as a Unix timestamp, so the whole rate-limit surface is
//! testable in microseconds rather than by sleeping. The service reads the
//! real clock once per request and hands it in.
//!
//! Persistence is a JSON journal (`FAUCET_STATE_FILE`), written after each
//! accepted drip via write-temp-then-rename — the same crash-safe idiom as the
//! maker's `.maker-state.json`. A restart that lost the journal would reset
//! every cooldown, which is the one bug that turns a rate limit into a
//! suggestion, so the journal is not optional in deployment (it is in the
//! tests, and for an ephemeral local demo).
//!
//! Not sqlite: at the faucet's scale the whole ledger is a few thousand map
//! entries, and rusqlite would add a C build dependency to a Rust workspace
//! that currently has none. If the ledger ever needs range queries or
//! concurrent writers, that trade flips.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Policy, format_wei_as_eth};

pub const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Per-address history. `total_wei` is what makes the lifetime cap survive
/// cooldown expiry: an address that comes back tomorrow is inside its
/// cooldown rules again but still spending the same lifetime allowance.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddressRecord {
    pub last_drip_at: u64,
    pub total_wei: u128,
    pub drips: u64,
}

/// Why a claim was refused. Each variant carries what the UI needs to say
/// something true and specific — "try again in 3 hours", not "rate limited".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    AddressCooldown { retry_after_secs: u64 },
    IpCooldown { retry_after_secs: u64 },
    LifetimeCap { already_wei: u128, cap_wei: u128 },
    DailyBudget { spent_wei: u128, budget_wei: u128 },
}

impl Refusal {
    /// A stable machine-readable tag, so the app can branch on the reason
    /// without matching on prose.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::AddressCooldown { .. } => "address_cooldown",
            Refusal::IpCooldown { .. } => "ip_cooldown",
            Refusal::LifetimeCap { .. } => "lifetime_cap",
            Refusal::DailyBudget { .. } => "daily_budget",
        }
    }

    /// The sentence a user should read. Written for the person in the Setup
    /// step, not for the operator — the operator has the logs.
    pub fn message(&self) -> String {
        match self {
            Refusal::AddressCooldown { retry_after_secs } => format!(
                "This address already got test ETH recently. Try again in {}.",
                humanize(*retry_after_secs)
            ),
            Refusal::IpCooldown { retry_after_secs } => format!(
                "This network already claimed recently. Try again in {}, or use one of the \
                 external faucets below.",
                humanize(*retry_after_secs)
            ),
            Refusal::LifetimeCap {
                already_wei,
                cap_wei,
            } => format!(
                "This address has already received {} ETH, the {} ETH lifetime limit. Use one of \
                 the external faucets below.",
                format_wei_as_eth(*already_wei),
                format_wei_as_eth(*cap_wei)
            ),
            Refusal::DailyBudget { budget_wei, .. } => format!(
                "The faucet has given out its {} ETH for today. Try again tomorrow, or use one of \
                 the external faucets below.",
                format_wei_as_eth(*budget_wei)
            ),
        }
    }
}

fn humanize(secs: u64) -> String {
    if secs >= 7200 {
        format!("{} hours", secs.div_ceil(3600))
    } else if secs >= 120 {
        format!("{} minutes", secs.div_ceil(60))
    } else if secs > 1 {
        format!("{secs} seconds")
    } else {
        "a moment".to_string()
    }
}

/// The undo record for a spend that has been committed but whose transaction
/// has not landed yet. Opaque to callers: hold it across the send, then either
/// drop it (success) or hand it to [`Ledger::rollback`] (failure).
#[derive(Debug, Clone)]
pub struct Reservation {
    address: String,
    ip: String,
    amount_wei: u128,
    previous_address: Option<AddressRecord>,
    previous_ip_at: Option<u64>,
    previous_day_index: u64,
    previous_day_spent_wei: u128,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ledger {
    /// Keyed by lowercase 0x-prefixed address. Normalization happens on the
    /// way in (see [`normalize_address`]) so a checksummed and a lowercase
    /// spelling of the same address can never hold two separate cooldowns.
    pub addresses: HashMap<String, AddressRecord>,
    /// Last drip per client IP. No history kept beyond the timestamp: the IP
    /// limit is a speed bump, and retaining more would be storing requester
    /// identity for no defensive gain.
    pub ips: HashMap<String, u64>,
    /// Which UTC day `day_spent_wei` refers to (`now / 86400`). Rolling the
    /// day is lazy — checked on read — so a faucet that sat idle overnight
    /// does not need a timer to get its budget back.
    pub day_index: u64,
    pub day_spent_wei: u128,
    /// Lifetime counters, for `/stats`.
    pub drips_served: u64,
    pub total_dripped_wei: u128,
}

/// Lowercase, 0x-prefixed, syntactically checked. Returns the canonical form
/// or an error naming what is wrong — the service uses this as its address
/// validator too, so a malformed address is refused before any RPC call.
pub fn normalize_address(address: &str) -> Result<String, String> {
    let trimmed = address.trim();
    let body = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X"));
    let Some(body) = body else {
        return Err("address must start with 0x".to_string());
    };
    if body.len() != 40 {
        return Err(format!(
            "address must be 40 hex characters after 0x (got {})",
            body.len()
        ));
    }
    if !body.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("address contains non-hex characters".to_string());
    }
    Ok(format!("0x{}", body.to_ascii_lowercase()))
}

impl Ledger {
    /// Would a drip of `policy.drip_wei` to `address` from `ip` be allowed
    /// right now? Checks in cheapest-and-most-specific-first order so the
    /// refusal a user sees names the rule that actually stopped them.
    ///
    /// `address` must already be normalized; `ip` may be any stable string
    /// (an empty one skips the IP rule, which is how a deployment behind a
    /// proxy that strips the client IP degrades — to no IP limit, not to a
    /// single shared one that would lock every user out after one claim).
    pub fn check(
        &self,
        address: &str,
        ip: &str,
        now: u64,
        policy: &Policy,
    ) -> Result<(), Refusal> {
        if let Some(record) = self.addresses.get(address) {
            if let Some(remaining) =
                remaining_cooldown(record.last_drip_at, now, policy.address_cooldown_secs)
            {
                return Err(Refusal::AddressCooldown {
                    retry_after_secs: remaining,
                });
            }
            // Saturating: a cap that was lowered below an address's existing
            // total must refuse, not wrap into a huge allowance.
            if record.total_wei.saturating_add(policy.drip_wei) > policy.lifetime_cap_wei {
                return Err(Refusal::LifetimeCap {
                    already_wei: record.total_wei,
                    cap_wei: policy.lifetime_cap_wei,
                });
            }
        }

        if !ip.is_empty()
            && let Some(last) = self.ips.get(ip)
            && let Some(remaining) = remaining_cooldown(*last, now, policy.ip_cooldown_secs)
        {
            return Err(Refusal::IpCooldown {
                retry_after_secs: remaining,
            });
        }

        let spent = self.spent_today(now);
        if spent.saturating_add(policy.drip_wei) > policy.daily_budget_wei {
            return Err(Refusal::DailyBudget {
                spent_wei: spent,
                budget_wei: policy.daily_budget_wei,
            });
        }

        Ok(())
    }

    /// What this UTC day has spent, accounting for a day roll that no timer
    /// has observed yet.
    pub fn spent_today(&self, now: u64) -> u128 {
        if now / SECONDS_PER_DAY == self.day_index {
            self.day_spent_wei
        } else {
            0
        }
    }

    /// Commit a drip. Call this only after the send succeeded: a recorded
    /// drip that never landed would cost a real user their daily claim, and
    /// the failure modes here (RPC timeouts) are common enough for that to
    /// matter more than the reverse race.
    pub fn record(&mut self, address: &str, ip: &str, now: u64, amount_wei: u128) {
        let day = now / SECONDS_PER_DAY;
        if day != self.day_index {
            self.day_index = day;
            self.day_spent_wei = 0;
        }
        self.day_spent_wei = self.day_spent_wei.saturating_add(amount_wei);

        let record = self.addresses.entry(address.to_string()).or_default();
        record.last_drip_at = now;
        record.total_wei = record.total_wei.saturating_add(amount_wei);
        record.drips += 1;

        if !ip.is_empty() {
            self.ips.insert(ip.to_string(), now);
        }

        self.drips_served += 1;
        self.total_dripped_wei = self.total_dripped_wei.saturating_add(amount_wei);
    }

    /// Reserve a drip: run [`check`](Self::check) and, if it passes, commit
    /// the spend immediately and hand back what it takes to undo it.
    ///
    /// The reserve-then-send order is what makes two simultaneous requests for
    /// the same address safe: the second one sees the first's spend and is
    /// refused, instead of both passing `check` and both sending. The send is
    /// then made OUTSIDE the lock (it waits on a chain receipt, and holding a
    /// global lock across that would serialize the whole service behind one
    /// slow RPC), and a failed send is undone with [`rollback`](Self::rollback)
    /// so a user is never charged a cooldown for ETH that never arrived.
    pub fn reserve(
        &mut self,
        address: &str,
        ip: &str,
        now: u64,
        policy: &Policy,
    ) -> Result<Reservation, Refusal> {
        self.check(address, ip, now, policy)?;
        let reservation = Reservation {
            address: address.to_string(),
            ip: ip.to_string(),
            amount_wei: policy.drip_wei,
            previous_address: self.addresses.get(address).cloned(),
            previous_ip_at: self.ips.get(ip).copied(),
            previous_day_index: self.day_index,
            previous_day_spent_wei: self.day_spent_wei,
        };
        self.record(address, ip, now, policy.drip_wei);
        Ok(reservation)
    }

    /// Undo a [`reserve`](Self::reserve) whose send failed, restoring exactly
    /// the prior state (including the day bucket, which `record` may have
    /// rolled).
    pub fn rollback(&mut self, reservation: Reservation) {
        match reservation.previous_address {
            Some(record) => {
                self.addresses.insert(reservation.address, record);
            }
            None => {
                self.addresses.remove(&reservation.address);
            }
        }
        if !reservation.ip.is_empty() {
            match reservation.previous_ip_at {
                Some(at) => {
                    self.ips.insert(reservation.ip, at);
                }
                None => {
                    self.ips.remove(&reservation.ip);
                }
            }
        }
        self.day_index = reservation.previous_day_index;
        self.day_spent_wei = reservation.previous_day_spent_wei;
        self.drips_served = self.drips_served.saturating_sub(1);
        self.total_dripped_wei = self.total_dripped_wei.saturating_sub(reservation.amount_wei);
    }

    /// Drop entries that can no longer refuse anything, so a long-lived
    /// faucet's journal tracks its active users rather than growing forever.
    /// Address records are kept as long as the LIFETIME cap could still bite;
    /// only IP entries (which carry no lifetime rule) are truly expirable.
    pub fn prune(&mut self, now: u64, policy: &Policy) {
        self.ips
            .retain(|_, last| remaining_cooldown(*last, now, policy.ip_cooldown_secs).is_some());
        self.addresses.retain(|_, record| {
            remaining_cooldown(record.last_drip_at, now, policy.address_cooldown_secs).is_some()
                || record.total_wei.saturating_add(policy.drip_wei) > policy.lifetime_cap_wei
        });
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map_err(|e| format!("{} is not a readable faucet journal: {e}", path.display())),
            // A missing journal is a first run, not an error. A journal that
            // exists but cannot be READ is an error: silently starting from
            // zero would hand every rate-limited address a fresh allowance.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// Write-temp-then-rename, so a crash mid-write leaves the previous
    /// journal intact rather than a truncated one that fails to parse on the
    /// next boot (and would then, per `load`, refuse to start).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let tmp: PathBuf = path.with_extension("tmp");
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|e| format!("cannot serialize the faucet journal: {e}"))?;
        std::fs::write(&tmp, encoded).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("cannot replace {}: {e}", path.display()))
    }
}

/// Seconds still to wait, or `None` when the cooldown has elapsed. A
/// zero-length cooldown is always elapsed, which is what makes
/// `FAUCET_IP_COOLDOWN_SECS=0` a clean way to turn the IP rule off.
fn remaining_cooldown(last: u64, now: u64, cooldown_secs: u64) -> Option<u64> {
    if cooldown_secs == 0 {
        return None;
    }
    let ready_at = last.saturating_add(cooldown_secs);
    // `now < last` means the clock went backwards (NTP step, restored
    // snapshot). Treat that as "still cooling down" rather than as an
    // instant reset: a rate limit that a clock skew can clear is not one.
    (now < ready_at).then(|| ready_at.saturating_sub(now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WEI_PER_ETH;

    const DRIP: u128 = WEI_PER_ETH / 50; // 0.02 ETH
    const ADDR: &str = "0x000000000000000000000000000000000000dead";
    const OTHER: &str = "0x000000000000000000000000000000000000beef";
    const IP: &str = "203.0.113.7";

    fn policy() -> Policy {
        Policy {
            drip_wei: DRIP,
            address_cooldown_secs: SECONDS_PER_DAY,
            ip_cooldown_secs: 3600,
            lifetime_cap_wei: DRIP * 5,
            daily_budget_wei: WEI_PER_ETH,
            max_recipient_balance_wei: DRIP,
        }
    }

    #[test]
    fn a_fresh_address_is_allowed() {
        let ledger = Ledger::default();
        assert_eq!(ledger.check(ADDR, IP, 1_000_000, &policy()), Ok(()));
    }

    #[test]
    fn a_second_claim_inside_the_cooldown_is_refused_with_the_wait() {
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(ADDR, IP, now, DRIP);

        let err = ledger.check(ADDR, IP, now + 3600, &policy()).unwrap_err();
        assert_eq!(
            err,
            Refusal::AddressCooldown {
                retry_after_secs: SECONDS_PER_DAY - 3600
            }
        );
        assert_eq!(err.code(), "address_cooldown");
        assert!(err.message().contains("23 hours"), "got: {}", err.message());
    }

    #[test]
    fn the_address_cooldown_clears_exactly_when_it_expires() {
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(ADDR, IP, now, DRIP);
        let p = policy();

        assert!(ledger.check(ADDR, "", now + SECONDS_PER_DAY - 1, &p).is_err());
        // At the boundary itself the cooldown is over — an off-by-one here
        // would silently add a second to every user's wait.
        assert_eq!(ledger.check(ADDR, "", now + SECONDS_PER_DAY, &p), Ok(()));
    }

    #[test]
    fn the_ip_cooldown_stops_a_different_address_from_the_same_client() {
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(ADDR, IP, now, DRIP);

        // The whole point of the IP rule: a fresh address does not reset it.
        let err = ledger.check(OTHER, IP, now + 60, &policy()).unwrap_err();
        assert_eq!(
            err,
            Refusal::IpCooldown {
                retry_after_secs: 3540
            }
        );
        // ...and a different client is unaffected.
        assert_eq!(ledger.check(OTHER, "198.51.100.4", now + 60, &policy()), Ok(()));
    }

    #[test]
    fn an_empty_ip_skips_the_ip_rule_entirely() {
        // A proxy that strips the client IP must degrade to "no IP limit",
        // never to one shared bucket that locks out every user after one claim.
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(ADDR, "", now, DRIP);
        assert_eq!(ledger.check(OTHER, "", now + 1, &policy()), Ok(()));
    }

    #[test]
    fn a_zero_ip_cooldown_disables_the_rule() {
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(ADDR, IP, now, DRIP);
        let p = Policy {
            ip_cooldown_secs: 0,
            ..policy()
        };
        assert_eq!(ledger.check(OTHER, IP, now + 1, &p), Ok(()));
    }

    #[test]
    fn the_lifetime_cap_outlives_the_cooldown() {
        let mut ledger = Ledger::default();
        let p = policy(); // cap = 5 drips
        let mut now = 1_000_000;
        for _ in 0..5 {
            assert_eq!(ledger.check(ADDR, "", now, &p), Ok(()), "at t={now}");
            ledger.record(ADDR, "", now, DRIP);
            now += SECONDS_PER_DAY;
        }
        // Cooldown long expired, but the allowance is spent.
        let err = ledger.check(ADDR, "", now, &p).unwrap_err();
        assert_eq!(
            err,
            Refusal::LifetimeCap {
                already_wei: DRIP * 5,
                cap_wei: DRIP * 5
            }
        );
        assert_eq!(err.code(), "lifetime_cap");
    }

    #[test]
    fn lowering_the_cap_below_an_existing_total_refuses_instead_of_wrapping() {
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(ADDR, "", now, DRIP * 4);
        let tightened = Policy {
            lifetime_cap_wei: DRIP,
            ..policy()
        };
        assert!(matches!(
            ledger.check(ADDR, "", now + SECONDS_PER_DAY, &tightened),
            Err(Refusal::LifetimeCap { .. })
        ));
    }

    #[test]
    fn the_daily_budget_stops_everyone_not_just_the_spender() {
        let p = Policy {
            daily_budget_wei: DRIP * 3,
            ..policy()
        };
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        for i in 0..3 {
            let addr = format!("0x{:040x}", i + 1);
            assert_eq!(ledger.check(&addr, "", now, &p), Ok(()));
            ledger.record(&addr, "", now, DRIP);
        }
        let err = ledger.check(OTHER, "", now, &p).unwrap_err();
        assert_eq!(
            err,
            Refusal::DailyBudget {
                spent_wei: DRIP * 3,
                budget_wei: DRIP * 3
            }
        );
        assert_eq!(err.code(), "daily_budget");
    }

    #[test]
    fn the_daily_budget_resets_on_the_utc_day_roll_without_a_timer() {
        let p = Policy {
            daily_budget_wei: DRIP,
            address_cooldown_secs: 0,
            ip_cooldown_secs: 0,
            ..policy()
        };
        let mut ledger = Ledger::default();
        // Just before midnight UTC.
        let now = 10 * SECONDS_PER_DAY - 10;
        ledger.record(ADDR, "", now, DRIP);
        assert!(ledger.check(ADDR, "", now, &p).is_err());

        let tomorrow = 10 * SECONDS_PER_DAY + 1;
        assert_eq!(ledger.spent_today(tomorrow), 0);
        assert_eq!(ledger.check(ADDR, "", tomorrow, &p), Ok(()));
        // Lifetime counters are NOT reset by the day roll.
        assert_eq!(ledger.drips_served, 1);
        assert_eq!(ledger.total_dripped_wei, DRIP);
    }

    #[test]
    fn the_address_rule_is_checked_before_the_budget() {
        // Ordering matters for the message the user reads: "you already
        // claimed" is actionable, "the faucet is empty for today" is not, and
        // an address inside its cooldown deserves the former.
        let p = Policy {
            daily_budget_wei: DRIP,
            ..policy()
        };
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(ADDR, IP, now, DRIP);
        assert!(matches!(
            ledger.check(ADDR, IP, now + 60, &p),
            Err(Refusal::AddressCooldown { .. })
        ));
    }

    #[test]
    fn a_backwards_clock_does_not_clear_a_cooldown() {
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(ADDR, IP, now, DRIP);
        // NTP steps the clock back an hour; the cooldown must survive it.
        assert!(matches!(
            ledger.check(ADDR, IP, now - 3600, &policy()),
            Err(Refusal::AddressCooldown { .. })
        ));
    }

    #[test]
    fn addresses_are_normalized_so_case_cannot_split_a_cooldown() {
        let checksummed = "0x000000000000000000000000000000000000DEAD";
        assert_eq!(normalize_address(checksummed).unwrap(), ADDR);
        // Surrounding whitespace and an uppercase 0X prefix are both accepted
        // — a user pasting from a block explorer produces either.
        assert_eq!(
            normalize_address(&format!("  0X{}  ", "DEAD".repeat(10))).unwrap(),
            format!("0x{}", "dead".repeat(10))
        );

        let mut ledger = Ledger::default();
        let now = 1_000_000;
        ledger.record(&normalize_address(checksummed).unwrap(), "", now, DRIP);
        assert!(matches!(
            ledger.check(&normalize_address(ADDR).unwrap(), "", now + 60, &policy()),
            Err(Refusal::AddressCooldown { .. })
        ));
    }

    #[test]
    fn malformed_addresses_are_refused_with_a_reason() {
        assert!(normalize_address("dead").unwrap_err().contains("0x"));
        assert!(normalize_address("0xdead").unwrap_err().contains("40 hex"));
        assert!(
            normalize_address(&format!("0x{}", "z".repeat(40)))
                .unwrap_err()
                .contains("non-hex")
        );
    }

    #[test]
    fn prune_keeps_capped_addresses_and_drops_stale_ips() {
        let p = policy();
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        // Spent its whole lifetime allowance: must survive pruning, or the
        // cap would be resettable by waiting.
        ledger.record(ADDR, IP, now, DRIP * 5);
        // A one-off claimant whose cooldown will have expired.
        ledger.record(OTHER, "198.51.100.4", now, DRIP);

        ledger.prune(now + 2 * SECONDS_PER_DAY, &p);
        assert!(ledger.addresses.contains_key(ADDR));
        assert!(!ledger.addresses.contains_key(OTHER));
        assert!(ledger.ips.is_empty());
        // Pruning is bookkeeping, never an amnesty.
        assert!(matches!(
            ledger.check(ADDR, "", now + 2 * SECONDS_PER_DAY, &p),
            Err(Refusal::LifetimeCap { .. })
        ));
    }

    #[test]
    fn a_reservation_blocks_a_concurrent_second_claim() {
        // The race this exists to stop: two requests for one address both pass
        // `check` before either records, and both send.
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        let _first = ledger.reserve(ADDR, IP, now, &policy()).unwrap();
        assert!(matches!(
            ledger.reserve(ADDR, IP, now, &policy()),
            Err(Refusal::AddressCooldown { .. })
        ));
    }

    #[test]
    fn rolling_back_a_failed_send_restores_the_prior_state_exactly() {
        let mut ledger = Ledger::default();
        let now = 1_000_000;
        let before = ledger.clone();

        let reservation = ledger.reserve(ADDR, IP, now, &policy()).unwrap();
        assert_ne!(ledger, before);
        ledger.rollback(reservation);

        // A user whose drip never landed must be able to try again at once —
        // charging them a 24 h cooldown for a failed RPC is the worst possible
        // way to fail.
        assert_eq!(ledger, before);
        assert_eq!(ledger.check(ADDR, IP, now, &policy()), Ok(()));
    }

    #[test]
    fn rollback_restores_a_repeat_claimant_rather_than_erasing_them() {
        let mut ledger = Ledger::default();
        let first = 1_000_000;
        ledger.record(ADDR, IP, first, DRIP);
        let after_first = ledger.clone();

        let later = first + 2 * SECONDS_PER_DAY;
        let reservation = ledger.reserve(ADDR, IP, later, &policy()).unwrap();
        ledger.rollback(reservation);

        // The earlier claim's history — including its lifetime total — must
        // survive; only the failed attempt is undone.
        assert_eq!(ledger, after_first);
        assert_eq!(ledger.addresses[ADDR].total_wei, DRIP);
        assert_eq!(ledger.addresses[ADDR].drips, 1);
    }

    #[test]
    fn rollback_restores_the_day_bucket_across_a_midnight_roll() {
        let mut ledger = Ledger::default();
        let before_midnight = 10 * SECONDS_PER_DAY - 10;
        ledger.record(OTHER, "", before_midnight, DRIP);
        let before = ledger.clone();

        // The reserve rolls the day bucket; the rollback must roll it back.
        let reservation = ledger
            .reserve(ADDR, "", 10 * SECONDS_PER_DAY + 5, &policy())
            .unwrap();
        ledger.rollback(reservation);
        assert_eq!(ledger, before);
    }

    #[test]
    fn the_journal_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("eth-faucet-test-{}", std::process::id()));
        let path = dir.join("state.json");
        let _ = std::fs::remove_dir_all(&dir);

        // A missing journal is a clean first run.
        assert_eq!(Ledger::load(&path).unwrap(), Ledger::default());

        let mut ledger = Ledger::default();
        ledger.record(ADDR, IP, 1_000_000, DRIP);
        ledger.save(&path).unwrap();

        let reloaded = Ledger::load(&path).unwrap();
        assert_eq!(reloaded, ledger);
        // The cooldown must survive the restart — that is the whole reason
        // the journal exists.
        assert!(matches!(
            reloaded.check(ADDR, IP, 1_000_060, &policy()),
            Err(Refusal::AddressCooldown { .. })
        ));

        std::fs::write(&path, b"{ not json").unwrap();
        assert!(Ledger::load(&path).is_err(), "a corrupt journal must not read as empty");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
