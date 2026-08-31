//! Env-only configuration. Every knob the scout report named as a
//! drain-resistance control is here, with the report's suggested starting
//! values as defaults, so an operator can deploy without reading the code and
//! tune without rebuilding.
//!
//! The private key is env-only and never logged, never journalled, never
//! served: `GET /stats` publishes the faucet's *address* and balance (both
//! public on Etherscan anyway) and nothing else about the key.

use std::time::Duration;

/// Wei per ETH, as a `u128`. Every amount in this service is wei in `u128`:
/// the whole reserve is a handful of ETH (~1e19 wei) against a `u128` ceiling
/// of ~3.4e38, so there is no overflow risk and no need for `U256` arithmetic
/// outside the alloy boundary.
pub const WEI_PER_ETH: u128 = 1_000_000_000_000_000_000;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub rpc_url: String,
    pub private_key: String,
    pub state_file: Option<String>,

    pub policy: Policy,

    /// Difficulty the service issues, in leading zero bits. 24 bits is ~16.7M
    /// hashes: a few seconds on one core, which is the "the app is working for
    /// a moment" the Setup step can honestly narrate. Raise toward 26–27 for
    /// ~30–60 s if abuse shows up.
    pub difficulty_bits: u8,
    /// How long an issued challenge stays claimable. Long enough for a slow
    /// machine's solve, short enough that a farmed challenge is worthless.
    pub challenge_ttl: Duration,
}

/// The rate/budget rules. Split out from transport/key config because it is
/// the part the tests exercise (see `ledger.rs`) — a pure value with no
/// network, no clock and no key in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// How much one successful claim sends.
    pub drip_wei: u128,
    /// Minimum gap between two drips to the same address.
    pub address_cooldown_secs: u64,
    /// Minimum gap between two drips to the same client IP. A SECONDARY
    /// signal only (VPNs are free, and app users can share a NAT), which is
    /// why it is far shorter than the address cooldown: it exists to blunt a
    /// trivial script loop, never to be the gate.
    pub ip_cooldown_secs: u64,
    /// Total this service will ever send to one address.
    pub lifetime_cap_wei: u128,
    /// Hard stop per UTC day across ALL addresses. This is the real backstop:
    /// every other control just makes honest use cheap relative to abuse,
    /// while this one bounds the worst case no matter what gets past them.
    pub daily_budget_wei: u128,
    /// Refuse addresses that already hold at least this much — a UX refusal
    /// ("you already have gas"), not a sybil defense. 0 disables the check.
    pub max_recipient_balance_wei: u128,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            // 0.02 ETH — the task's stated drip. ~4x the "~0.005 ETH covers a
            // peer's gas" figure in docs/testnet.md, so a funded user gets
            // several swaps out of one claim rather than coming straight back.
            drip_wei: WEI_PER_ETH / 50,
            address_cooldown_secs: 24 * 60 * 60,
            ip_cooldown_secs: 60 * 60,
            // 0.1 ETH: five drips over the faucet's life for one address.
            lifetime_cap_wei: WEI_PER_ETH / 10,
            // 1 ETH/day. At 0.02/drip that is 50 honest users a day, and at
            // most 50 days of a 50 ETH reserve under continuous attack —
            // enough runway for the health check to be noticed and acted on.
            daily_budget_wei: WEI_PER_ETH,
            // Equal to the drip: "you already have at least one drip's worth".
            max_recipient_balance_wei: WEI_PER_ETH / 50,
        }
    }
}

/// Parse a decimal ETH string ("0.02", "1", ".5") into wei, without floats —
/// a float round-trip on an 18-decimal quantity is exactly the kind of quiet
/// off-by-a-few-wei that makes a budget check disagree with itself.
pub fn parse_eth_to_wei(input: &str) -> Result<u128, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("empty amount".to_string());
    }
    let (whole, frac) = match trimmed.split_once('.') {
        Some((w, f)) => (w, f),
        None => (trimmed, ""),
    };
    let whole = if whole.is_empty() { "0" } else { whole };
    if !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("'{input}' is not a decimal ETH amount"));
    }
    if frac.len() > 18 {
        return Err(format!("'{input}' has more than 18 decimal places"));
    }
    let whole: u128 = whole
        .parse()
        .map_err(|_| format!("'{input}' overflows a u128 of wei"))?;
    let mut padded = frac.to_string();
    while padded.len() < 18 {
        padded.push('0');
    }
    let frac: u128 = if padded.is_empty() {
        0
    } else {
        padded
            .parse()
            .map_err(|_| format!("'{input}' has an unparseable fraction"))?
    };
    whole
        .checked_mul(WEI_PER_ETH)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| format!("'{input}' overflows a u128 of wei"))
}

/// Render wei as a decimal ETH string with trailing zeros trimmed. Used only
/// for human-facing JSON fields; the machine-readable `*_wei` fields are
/// always served alongside so a client never has to parse this back.
pub fn format_wei_as_eth(wei: u128) -> String {
    let whole = wei / WEI_PER_ETH;
    let frac = wei % WEI_PER_ETH;
    if frac == 0 {
        return whole.to_string();
    }
    let frac = format!("{frac:018}");
    format!("{whole}.{}", frac.trim_end_matches('0'))
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn env_u64(key: &str, default: u64) -> Result<u64, String> {
    match env_opt(key) {
        Some(v) => v
            .trim()
            .parse()
            .map_err(|_| format!("{key}='{v}' is not a whole number")),
        None => Ok(default),
    }
}

fn env_eth(key: &str, default: u128) -> Result<u128, String> {
    match env_opt(key) {
        Some(v) => parse_eth_to_wei(&v).map_err(|e| format!("{key}: {e}")),
        None => Ok(default),
    }
}

impl Config {
    /// Read the whole configuration from the environment, failing with one
    /// actionable message per bad value rather than falling back silently —
    /// a faucet that quietly ran on default limits because `FAUCET_DRIP_ETH`
    /// had a typo is exactly the failure this refuses to have.
    pub fn from_env() -> Result<Self, String> {
        let defaults = Policy::default();

        let private_key = env_opt("FAUCET_PRIVATE_KEY").ok_or_else(|| {
            "FAUCET_PRIVATE_KEY is required (64 hex chars, with or without 0x). Generate a \
             THROWAWAY key with `cargo run -p eth-faucet --bin eth-faucet -- --genkey` and fund \
             it; never reuse a personal key."
                .to_string()
        })?;
        let rpc_url = env_opt("FAUCET_RPC_URL")
            .unwrap_or_else(|| "https://ethereum-sepolia-rpc.publicnode.com".to_string());
        if !rpc_url.starts_with("http://") && !rpc_url.starts_with("https://") {
            // The faucet only ever sends transactions and reads balances, both
            // plain HTTP JSON-RPC. A ws:// URL would fail at connect time with
            // a transport error; say so here instead.
            return Err(format!(
                "FAUCET_RPC_URL must be an http:// or https:// endpoint (got '{rpc_url}')"
            ));
        }

        let policy = Policy {
            drip_wei: env_eth("FAUCET_DRIP_ETH", defaults.drip_wei)?,
            address_cooldown_secs: env_u64(
                "FAUCET_ADDRESS_COOLDOWN_SECS",
                defaults.address_cooldown_secs,
            )?,
            ip_cooldown_secs: env_u64("FAUCET_IP_COOLDOWN_SECS", defaults.ip_cooldown_secs)?,
            lifetime_cap_wei: env_eth("FAUCET_LIFETIME_CAP_ETH", defaults.lifetime_cap_wei)?,
            daily_budget_wei: env_eth("FAUCET_DAILY_BUDGET_ETH", defaults.daily_budget_wei)?,
            max_recipient_balance_wei: env_eth(
                "FAUCET_MAX_RECIPIENT_BALANCE_ETH",
                defaults.max_recipient_balance_wei,
            )?,
        };

        if policy.drip_wei == 0 {
            return Err("FAUCET_DRIP_ETH must be greater than zero".to_string());
        }
        if policy.lifetime_cap_wei < policy.drip_wei {
            return Err(format!(
                "FAUCET_LIFETIME_CAP_ETH ({}) is below FAUCET_DRIP_ETH ({}) — no address could \
                 ever claim once",
                format_wei_as_eth(policy.lifetime_cap_wei),
                format_wei_as_eth(policy.drip_wei)
            ));
        }
        if policy.daily_budget_wei < policy.drip_wei {
            return Err(format!(
                "FAUCET_DAILY_BUDGET_ETH ({}) is below FAUCET_DRIP_ETH ({}) — the faucet would \
                 refuse every claim",
                format_wei_as_eth(policy.daily_budget_wei),
                format_wei_as_eth(policy.drip_wei)
            ));
        }

        let difficulty_bits = env_u64("FAUCET_POW_DIFFICULTY_BITS", 24)?;
        if difficulty_bits > u64::from(eth_faucet_pow::MAX_DIFFICULTY_BITS) {
            return Err(format!(
                "FAUCET_POW_DIFFICULTY_BITS={difficulty_bits} exceeds the {}-bit ceiling; a \
                 challenge that hard would take the app hours to solve",
                eth_faucet_pow::MAX_DIFFICULTY_BITS
            ));
        }

        Ok(Self {
            bind: env_opt("FAUCET_BIND").unwrap_or_else(|| "0.0.0.0:8787".to_string()),
            rpc_url,
            private_key,
            state_file: env_opt("FAUCET_STATE_FILE"),
            policy,
            difficulty_bits: difficulty_bits as u8,
            challenge_ttl: Duration::from_secs(env_u64("FAUCET_CHALLENGE_TTL_SECS", 300)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimal_eth_without_float_drift() {
        assert_eq!(parse_eth_to_wei("1").unwrap(), WEI_PER_ETH);
        assert_eq!(parse_eth_to_wei("0.02").unwrap(), 20_000_000_000_000_000);
        assert_eq!(parse_eth_to_wei(".5").unwrap(), WEI_PER_ETH / 2);
        assert_eq!(parse_eth_to_wei("0").unwrap(), 0);
        // 18 decimals is the full precision of wei and must survive exactly.
        assert_eq!(parse_eth_to_wei("0.000000000000000001").unwrap(), 1);
        assert_eq!(parse_eth_to_wei("  0.02  ").unwrap(), 20_000_000_000_000_000);
    }

    #[test]
    fn rejects_junk_amounts_rather_than_defaulting() {
        assert!(parse_eth_to_wei("").is_err());
        assert!(parse_eth_to_wei("abc").is_err());
        assert!(parse_eth_to_wei("1.2.3").is_err());
        assert!(parse_eth_to_wei("-1").is_err());
        assert!(parse_eth_to_wei("0.0000000000000000001").is_err());
    }

    #[test]
    fn formats_wei_back_to_a_readable_eth_string() {
        assert_eq!(format_wei_as_eth(0), "0");
        assert_eq!(format_wei_as_eth(WEI_PER_ETH), "1");
        assert_eq!(format_wei_as_eth(20_000_000_000_000_000), "0.02");
        assert_eq!(format_wei_as_eth(1), "0.000000000000000001");
    }

    #[test]
    fn eth_amounts_round_trip_through_both_directions() {
        // Canonical forms only — the point is that the pair is lossless for
        // every amount /stats will ever print back to an operator.
        for s in ["0", "1", "0.02", "0.5", "123.456", "0.000000000000000001"] {
            assert_eq!(format_wei_as_eth(parse_eth_to_wei(s).unwrap()), s);
        }
    }
}
