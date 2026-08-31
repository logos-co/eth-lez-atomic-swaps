//! Client for the in-house Sepolia drip faucet (`eth-faucet/`).
//!
//! Three steps, all behind one blocking call the app makes on a worker thread:
//! ask for a challenge, burn some CPU solving it, post the answer. The solver
//! is `eth-faucet-pow`, the same crate the service verifies with, so a change
//! to the hash rule cannot land on one side only.
//!
//! Everything the app shows the user comes back as JSON: a `code` it can
//! branch on and a `message` written for the person in the Setup step. The
//! service authors the message for its own refusals (cooldowns, budget) —
//! only it knows how long the wait is — and this module authors the ones for
//! failures the service never sees (unreachable, malformed answer).

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde::Deserialize;

/// How long any single HTTP leg may take. Generous for a VPS over a slow
/// link, far short of the app's own FFI call timeout.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling on the solve, independent of what the challenge's expiry implies.
/// A service that issued an unreasonably long TTL must not be able to pin the
/// app's CPU for it.
const MAX_SOLVE: Duration = Duration::from_secs(180);

/// The successful outcome, plus every refusal, in one shape. Serialized
/// straight across the FFI boundary.
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum FaucetResult {
    Dripped {
        outcome: &'static str,
        tx_hash: String,
        address: String,
        amount_eth: String,
        chain_id: u64,
    },
    Refused {
        error: String,
        code: String,
    },
}

impl FaucetResult {
    fn refused(code: &str, error: impl Into<String>) -> Self {
        FaucetResult::Refused {
            error: error.into(),
            code: code.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct ChallengeBody {
    seed: String,
    difficulty_bits: u8,
    expires_at: u64,
}

#[derive(Deserialize)]
struct DripBody {
    tx_hash: String,
    address: String,
    amount_eth: String,
    chain_id: u64,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Trim a trailing slash so `http://host:8787/` and `http://host:8787` both
/// build the same URLs.
fn base_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

/// Read a non-2xx response as the service's typed refusal, falling back to the
/// status line when the body is not one. The fallback matters: a reverse proxy
/// returning its own 502 HTML page must still produce a sentence, not a raw
/// parse error.
async fn refusal_from(response: reqwest::Response) -> FaucetResult {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    match serde_json::from_str::<ErrorBody>(&body) {
        Ok(parsed) => FaucetResult::refused(&parsed.error.code, parsed.error.message),
        Err(_) => FaucetResult::refused(
            "faucet_error",
            format!("The faucet refused the request ({status})."),
        ),
    }
}

/// Ask the faucet for test ETH for `address`. Blocking and CPU-bound (the PoW
/// solve): call it from a worker thread, never a UI thread.
pub async fn request_eth(faucet_url: &str, address: &str) -> FaucetResult {
    let base = base_url(faucet_url);
    if base.is_empty() {
        return FaucetResult::refused(
            "not_configured",
            "No faucet is configured for this build. Use one of the external faucets below.",
        );
    }

    let client = match reqwest::Client::builder().timeout(HTTP_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return FaucetResult::refused("client_error", format!("HTTP client error: {e}")),
    };

    // --- 1. Challenge ---
    let challenge: ChallengeBody = {
        let response = match client
            .get(format!("{base}/challenge"))
            .query(&[("address", address)])
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return FaucetResult::refused(
                    "unreachable",
                    format!(
                        "Could not reach the faucet at {base}: {e}. Use one of the external \
                         faucets below."
                    ),
                );
            }
        };
        if !response.status().is_success() {
            return refusal_from(response).await;
        }
        match response.json().await {
            Ok(c) => c,
            Err(e) => {
                return FaucetResult::refused(
                    "bad_response",
                    format!("The faucet's answer could not be read: {e}"),
                );
            }
        }
    };

    // --- 2. Solve ---
    let seed = match (eth_faucet_pow::Challenge {
        address: address.to_string(),
        seed: challenge.seed.clone(),
        difficulty_bits: challenge.difficulty_bits,
        expires_at: challenge.expires_at,
    })
    .seed_bytes()
    {
        Ok(s) => s,
        Err(e) => return FaucetResult::refused("bad_response", format!("The faucet's puzzle is malformed: {e}")),
    };

    // Give up before the challenge expires — a solution that arrives late is
    // wasted CPU and a confusing error, so stop while there is still time to
    // say something better.
    let until_expiry = Duration::from_secs(challenge.expires_at.saturating_sub(now_secs()));
    if until_expiry.is_zero() {
        return FaucetResult::refused(
            "challenge_expired",
            "The faucet's puzzle expired before it arrived — try again.",
        );
    }
    let budget = until_expiry.min(MAX_SOLVE);
    let difficulty_bits = challenge.difficulty_bits;

    let solution = match tokio::task::spawn_blocking(move || {
        let cancel = AtomicBool::new(false);
        eth_faucet_pow::compute_solution_bounded(
            &seed,
            difficulty_bits,
            eth_faucet_pow::MAX_POW_ITERATIONS,
            Some(Instant::now() + budget),
            &cancel,
        )
    })
    .await
    {
        Ok(Ok(solution)) => solution,
        Ok(Err(e)) => return FaucetResult::refused("pow_failed", e),
        Err(e) => return FaucetResult::refused("pow_failed", format!("PoW task failed: {e}")),
    };

    // --- 3. Claim ---
    let response = match client
        .post(format!("{base}/drip"))
        .json(&serde_json::json!({
            "address": address,
            // A string, not a number: a u128 does not survive every JSON
            // stack intact, and a solution that quietly loses its low bits
            // would look like a faucet rejecting a correct answer.
            "pow_solution": solution.to_string(),
        }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return FaucetResult::refused(
                "unreachable",
                format!("The faucet stopped answering while claiming: {e}"),
            );
        }
    };
    if !response.status().is_success() {
        return refusal_from(response).await;
    }

    match response.json::<DripBody>().await {
        Ok(drip) => FaucetResult::Dripped {
            outcome: "Dripped",
            tx_hash: drip.tx_hash,
            address: drip.address,
            amount_eth: drip.amount_eth,
            chain_id: drip.chain_id,
        },
        Err(e) => FaucetResult::refused(
            "bad_response",
            format!(
                "The faucet sent the ETH but its answer could not be read ({e}) — check your \
                 balance before claiming again."
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_normalizes_trailing_slashes_and_whitespace() {
        assert_eq!(base_url("  http://host:8787/  "), "http://host:8787");
        assert_eq!(base_url("http://host:8787"), "http://host:8787");
        assert_eq!(base_url("///"), "");
    }

    #[tokio::test]
    async fn an_unconfigured_url_refuses_without_a_network_call() {
        let result = request_eth("   ", "0x0000000000000000000000000000000000000001").await;
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["code"], "not_configured");
    }

    #[test]
    fn a_drip_serializes_flat_for_the_ffi_boundary() {
        let json = serde_json::to_value(FaucetResult::Dripped {
            outcome: "Dripped",
            tx_hash: "0xabc".into(),
            address: "0xdef".into(),
            amount_eth: "0.02".into(),
            chain_id: 11155111,
        })
        .unwrap();
        assert_eq!(json["outcome"], "Dripped");
        assert_eq!(json["tx_hash"], "0xabc");
        // No `error` key on success: the C++ side treats any `error` as a
        // failure, so an untagged enum leaking one would invert the outcome.
        assert!(json.get("error").is_none());
    }

    #[test]
    fn a_refusal_carries_both_a_code_and_a_message() {
        let json = serde_json::to_value(FaucetResult::refused(
            "address_cooldown",
            "Try again in 23 hours.",
        ))
        .unwrap();
        assert_eq!(json["code"], "address_cooldown");
        assert_eq!(json["error"], "Try again in 23 hours.");
        assert!(json.get("tx_hash").is_none());
    }
}
