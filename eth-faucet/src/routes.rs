//! The HTTP surface: `/challenge`, `/drip`, `/health`, `/stats`.
//!
//! Every refusal is a typed JSON body with a stable `code` and a
//! user-readable `message`, so the app can put the faucet's own sentence in
//! the Setup card without inventing prose for a condition it cannot see.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;

use crate::chain::{Chain, eth_for_logs};
use crate::challenges::Challenges;
use crate::config::{Config, format_wei_as_eth};
use crate::ledger::{Ledger, normalize_address};

/// How long a balance read may take before `/health` and `/stats` give up and
/// report the RPC as unreachable. Without this a wedged RPC would hang the
/// health check itself, which is the one endpoint that must always answer.
const BALANCE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct AppState {
    pub config: Config,
    pub chain: Chain,
    /// One lock over the ledger. Held only for the check/reserve/rollback
    /// critical sections — never across a chain send (see `Ledger::reserve`).
    pub ledger: tokio::sync::Mutex<Ledger>,
    pub challenges: tokio::sync::Mutex<Challenges>,
    pub started_at: u64,
}

pub type Shared = Arc<AppState>;

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/challenge", get(challenge))
        .route("/drip", post(drip))
        .route("/health", get(health))
        .route("/stats", get(stats))
        .with_state(state)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A refusal, rendered the same way everywhere: an HTTP status plus
/// `{"error": {"code", "message"}}`.
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

/// The client IP, preferring `X-Forwarded-For`'s first hop.
///
/// Trusting that header is only sound behind a reverse proxy that sets it, and
/// the IP limit is explicitly a secondary signal (a spoofed header buys an
/// attacker nothing the address cooldown and the daily budget do not still
/// enforce), so this trades a strictly-correct-but-useless-behind-nginx
/// implementation for a useful one.
fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    if let Some(forwarded) = headers.get("x-forwarded-for")
        && let Ok(value) = forwarded.to_str()
        && let Some(first) = value.split(',').next()
        && !first.trim().is_empty()
    {
        return first.trim().to_string();
    }
    peer.map(|addr| addr.ip().to_string()).unwrap_or_default()
}

#[derive(Deserialize)]
struct ChallengeQuery {
    address: String,
}

#[derive(Serialize)]
struct ChallengeResponse {
    address: String,
    seed: String,
    difficulty_bits: u8,
    expires_at: u64,
    /// Roughly how many hashes the client should expect to try. Purely
    /// informational — it lets the app show an honest "about N seconds"
    /// instead of an indefinite spinner.
    expected_hashes: u64,
    drip_wei: String,
    drip_eth: String,
}

/// `GET /challenge?address=0x…`
///
/// Issuing does NOT consume any allowance and does not check cooldowns: a user
/// who is rate-limited should learn that from `/drip`'s typed refusal after a
/// solve, not from a challenge endpoint that leaks the ledger's contents to
/// anyone who asks about an address.
async fn challenge(
    State(state): State<Shared>,
    Query(query): Query<ChallengeQuery>,
) -> Result<Json<ChallengeResponse>, ApiError> {
    let address = normalize_address(&query.address)
        .map_err(|e| ApiError::bad_request("bad_address", e))?;

    let now = now_secs();
    let mut challenges = state.challenges.lock().await;
    challenges.prune(now);
    let issued = challenges.issue(
        &address,
        state.config.difficulty_bits,
        state.config.challenge_ttl,
        now,
    );

    Ok(Json(ChallengeResponse {
        address: issued.address,
        seed: issued.seed,
        difficulty_bits: issued.difficulty_bits,
        expires_at: issued.expires_at,
        expected_hashes: eth_faucet_pow::expected_hashes(issued.difficulty_bits) as u64,
        drip_wei: state.config.policy.drip_wei.to_string(),
        drip_eth: format_wei_as_eth(state.config.policy.drip_wei),
    }))
}

#[derive(Deserialize)]
struct DripRequest {
    address: String,
    /// The PoW answer. Accepted as a JSON string because a `u128` does not
    /// survive every JSON stack's number handling intact (JavaScript's
    /// included) — and a solution that silently loses its low bits in transit
    /// would look, to the user, like a faucet that rejects correct answers.
    pow_solution: String,
}

#[derive(Serialize)]
struct DripResponse {
    tx_hash: String,
    address: String,
    amount_wei: String,
    amount_eth: String,
    chain_id: u64,
}

/// `POST /drip {"address": "0x…", "pow_solution": "12345"}`
async fn drip(
    State(state): State<Shared>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Json(request): Json<DripRequest>,
) -> Result<Json<DripResponse>, ApiError> {
    let address = normalize_address(&request.address)
        .map_err(|e| ApiError::bad_request("bad_address", e))?;
    let solution: u128 = request.pow_solution.trim().parse().map_err(|_| {
        ApiError::bad_request(
            "bad_solution",
            "pow_solution must be a decimal integer sent as a JSON string",
        )
    })?;

    let ip = client_ip(&headers, peer.map(|ConnectInfo(addr)| addr));
    let now = now_secs();

    // 1. The PoW gate, first: it is the only check that costs the CLIENT
    //    anything, so making it the price of admission to the rest is what
    //    keeps probing the ledger cheap for us and expensive for an attacker.
    state
        .challenges
        .lock()
        .await
        .redeem(&address, solution, now)
        .map_err(|e| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: e.code(),
            message: e.message(),
        })?;

    // 2. "You already have gas" — a UX refusal, not a sybil defense (a sybil
    //    just uses empty addresses). Deliberately after the PoW so it cannot
    //    be used as a free balance oracle.
    let recipient: alloy::primitives::Address = address
        .parse()
        .map_err(|_| ApiError::bad_request("bad_address", "unparseable address"))?;
    let cap = state.config.policy.max_recipient_balance_wei;
    if cap > 0 {
        match tokio::time::timeout(BALANCE_TIMEOUT, state.chain.balance_wei(recipient)).await {
            Ok(Ok(balance)) if balance >= cap => {
                return Err(ApiError {
                    status: StatusCode::BAD_REQUEST,
                    code: "already_funded",
                    message: format!(
                        "This address already holds {} Sepolia ETH — enough for gas. The faucet \
                         saves its drips for empty accounts.",
                        format_wei_as_eth(balance)
                    ),
                });
            }
            Ok(Ok(_)) => {}
            // An unreadable balance must not block an honest claim: the
            // pinned public Sepolia RPC is known to be flaky in this project,
            // and the rate limits — not this check — are what bound the loss.
            Ok(Err(e)) => tracing::warn!(%address, error = %e, "recipient balance read failed; allowing"),
            Err(_) => tracing::warn!(%address, "recipient balance read timed out; allowing"),
        }
    }

    // 3. Rate limits. Reserve inside the lock so two simultaneous requests
    //    cannot both pass; send outside it.
    let reservation = {
        let mut ledger = state.ledger.lock().await;
        ledger
            .reserve(&address, &ip, now, &state.config.policy)
            .map_err(|refusal| ApiError {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: refusal.code(),
                message: refusal.message(),
            })?
    };

    let amount = state.config.policy.drip_wei;
    match state.chain.send_drip(recipient, amount).await {
        Ok(tx_hash) => {
            let mut ledger = state.ledger.lock().await;
            ledger.prune(now, &state.config.policy);
            if let Some(path) = &state.config.state_file
                && let Err(e) = ledger.save(std::path::Path::new(path))
            {
                // The drip DID land; failing the request now would tell the
                // user it did not. Log loudly — a journal that stopped saving
                // means cooldowns will not survive a restart.
                tracing::error!(error = %e, "faucet journal write failed after a successful drip");
            }
            drop(ledger);

            tracing::info!(
                %address, %tx_hash, amount = %eth_for_logs(amount),
                "dripped"
            );
            Ok(Json(DripResponse {
                tx_hash,
                address,
                amount_wei: amount.to_string(),
                amount_eth: format_wei_as_eth(amount),
                chain_id: state.chain.chain_id(),
            }))
        }
        Err(e) => {
            state.ledger.lock().await.rollback(reservation);
            tracing::error!(%address, error = %e, "drip send failed; reservation rolled back");
            Err(ApiError {
                status: StatusCode::BAD_GATEWAY,
                code: "send_failed",
                message: format!(
                    "The faucet could not send the transaction: {e}. Nothing was charged against \
                     your address — try again in a moment."
                ),
            })
        }
    }
}

/// `GET /health` — cheap enough for a container healthcheck, and honest about
/// what "unhealthy" means: the faucet cannot serve a full day's budget, or it
/// cannot reach the chain at all.
async fn health(State(state): State<Shared>) -> Response {
    let balance = tokio::time::timeout(
        BALANCE_TIMEOUT,
        state.chain.balance_wei(state.chain.address()),
    )
    .await;

    let (balance_wei, rpc_ok, rpc_error) = match balance {
        Ok(Ok(wei)) => (Some(wei), true, None),
        Ok(Err(e)) => (None, false, Some(e)),
        Err(_) => (
            None,
            false,
            Some(format!("balance read timed out after {BALANCE_TIMEOUT:?}")),
        ),
    };

    let budget = state.config.policy.daily_budget_wei;
    let days_of_runway = balance_wei.map(|wei| wei / budget.max(1));
    // Unhealthy below one full day of budget: that is the point at which the
    // faucet can no longer honour the promise its own limits make, and the
    // point at which an operator still has a day to refill it.
    let funded = days_of_runway.is_some_and(|days| days >= 1);
    let healthy = rpc_ok && funded;

    let body = json!({
        "status": if healthy { "ok" } else { "unhealthy" },
        "rpc_ok": rpc_ok,
        "rpc_error": rpc_error,
        "faucet_address": format!("{:#x}", state.chain.address()),
        "chain_id": state.chain.chain_id(),
        "balance_wei": balance_wei.map(|w| w.to_string()),
        "balance_eth": balance_wei.map(format_wei_as_eth),
        "days_of_budget_remaining": days_of_runway,
        "uptime_secs": now_secs().saturating_sub(state.started_at),
    });

    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

/// `GET /stats` — balance, drips served, and every limit currently in force,
/// so "why did it refuse me" is answerable without shell access to the VPS.
async fn stats(State(state): State<Shared>) -> Json<serde_json::Value> {
    let balance_wei = tokio::time::timeout(
        BALANCE_TIMEOUT,
        state.chain.balance_wei(state.chain.address()),
    )
    .await
    .ok()
    .and_then(|r| r.ok());

    let now = now_secs();
    let ledger = state.ledger.lock().await;
    let spent_today = ledger.spent_today(now);
    let policy = &state.config.policy;

    Json(json!({
        "faucet_address": format!("{:#x}", state.chain.address()),
        "chain_id": state.chain.chain_id(),
        "balance_wei": balance_wei.map(|w| w.to_string()),
        "balance_eth": balance_wei.map(format_wei_as_eth),
        "drips_served": ledger.drips_served,
        "total_dripped_eth": format_wei_as_eth(ledger.total_dripped_wei),
        "known_addresses": ledger.addresses.len(),
        "outstanding_challenges": state.challenges.lock().await.outstanding_count(),
        "today": {
            "spent_eth": format_wei_as_eth(spent_today),
            "budget_eth": format_wei_as_eth(policy.daily_budget_wei),
            "remaining_drips": policy
                .daily_budget_wei
                .saturating_sub(spent_today)
                / policy.drip_wei.max(1),
        },
        "policy": {
            "drip_eth": format_wei_as_eth(policy.drip_wei),
            "address_cooldown_secs": policy.address_cooldown_secs,
            "ip_cooldown_secs": policy.ip_cooldown_secs,
            "lifetime_cap_eth": format_wei_as_eth(policy.lifetime_cap_wei),
            "daily_budget_eth": format_wei_as_eth(policy.daily_budget_wei),
            "max_recipient_balance_eth": format_wei_as_eth(policy.max_recipient_balance_wei),
            "pow_difficulty_bits": state.config.difficulty_bits,
            "challenge_ttl_secs": state.config.challenge_ttl.as_secs(),
        },
        "uptime_secs": now.saturating_sub(state.started_at),
    }))
}
