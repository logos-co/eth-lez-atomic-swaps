//! The MCP server: five typed tools over the LEZ public testnet and the
//! Sepolia EthHTLC, with a version-fingerprint write gate.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lee::{AccountId, PrivateKey, PublicKey, PublicTransaction, public_transaction::WitnessSet};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use sequencer_service_protocol::{HashType, LeeTransaction};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use swap_orchestrator::{
    config::{LezAuth, account_id_to_base58, parse_base58_account_id},
    lez::client::LezClient,
};
use tokio::sync::{OnceCell, RwLock, Semaphore};

use crate::{
    config::McpConfig,
    eth::{EthReader, parse_bytes32, state_str},
    faucet,
    fingerprint::{FingerprintReport, redact_url, run_fingerprint},
};

/// How long to wait for a submitted LEZ effect to commit (public-testnet
/// blocks are ~30–60 s apart).
const COMMIT_TIMEOUT: Duration = Duration::from_secs(240);
const COMMIT_POLL: Duration = Duration::from_secs(5);

/// The write gate re-verifies the sequencer fingerprint before a submission if
/// the cached verification is older than this. Bounds getProgramIds load on
/// multi-claim loops while still catching a mid-session testnet upgrade.
const FINGERPRINT_TTL: Duration = Duration::from_secs(60);

/// Lifetime of a transfer preview token (server-issued, single-use).
const PREVIEW_TTL: Duration = Duration::from_secs(180);

/// Process-global cap on concurrent faucet PoW solves.
const POW_CONCURRENCY: usize = 2;

/// Hard cap on faucet claim iterations per call.
const MAX_FAUCET_CLAIMS: u32 = 20;

// ── Write gate ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Gate {
    /// Startup/last fingerprint matched — writes enabled.
    Verified(FingerprintReport),
    /// Fingerprint ran and mismatched — writes hard-refused.
    Mismatch(FingerprintReport),
    /// Fingerprint could not run (sequencer unreachable) — writes refused.
    Unverified { error: String },
}

/// The gate plus the instant it was last (re)verified against the configured
/// sequencer. Held under one lock so freshness and verdict move together.
struct GateState {
    gate: Gate,
    checked_at: Instant,
}

/// A write may ride the cached gate only if it is currently verified AND the
/// verification is younger than `ttl`. Anything else forces a refresh.
fn gate_is_fresh(gs: &GateState, ttl: Duration, now: Instant) -> bool {
    gs.gate.writes_enabled() && now.saturating_duration_since(gs.checked_at) < ttl
}

/// Single-flight, double-checked gate refresh. Holds `lock` so only one
/// verification runs at a time; re-checks freshness *after* acquiring the lock
/// so a caller that queued behind an in-flight refresh reuses its result rather
/// than launching a second one. Because run + store happen serially under the
/// lock, a slower/older response can never overwrite a newer verdict (no
/// completion-order resurrection of stale states). Returns the effective gate.
async fn refresh_gate_single_flight<F, Fut>(
    gate: &RwLock<GateState>,
    lock: &tokio::sync::Mutex<()>,
    ttl: Duration,
    refresh: F,
) -> Gate
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Gate>,
{
    let _flight = lock.lock().await;
    // Double-check: another refresh may have completed while we waited.
    {
        let gs = gate.read().await;
        if gate_is_fresh(&gs, ttl, Instant::now()) {
            return gs.gate.clone();
        }
    }
    let new_gate = refresh().await;
    let mut gs = gate.write().await;
    gs.gate = new_gate.clone();
    gs.checked_at = Instant::now();
    new_gate
}

/// A server-issued transfer preview, bound to the exact parameters it previewed
/// and consumed on first use. A caller cannot broadcast a transfer that was
/// never previewed (or previewed with different parameters).
struct PreviewToken {
    from: AccountId,
    to: AccountId,
    amount: u128,
    expires: Instant,
}

impl Gate {
    pub fn writes_enabled(&self) -> bool {
        matches!(self, Gate::Verified(_))
    }

    pub fn to_json(&self) -> Value {
        match self {
            Gate::Verified(r) => json!({"status": "verified", "report": r}),
            Gate::Mismatch(r) => json!({"status": "mismatch", "report": r}),
            Gate::Unverified { error } => json!({"status": "unverified", "error": error}),
        }
    }
}

/// Signing identity resolved at startup. The key never leaves this struct
/// and is never logged or echoed through tool results.
pub struct Signer {
    pub account_id: AccountId,
    key: PrivateKey,
}

impl Signer {
    pub fn from_auth(auth: &LezAuth) -> Result<Self, String> {
        match auth {
            LezAuth::RawKey(hex_key) => {
                let bytes: [u8; 32] = hex::decode(hex_key.trim())
                    .map_err(|e| format!("invalid LEZ signing key hex: {e}"))?
                    .try_into()
                    .map_err(|_| "LEZ signing key must be 32 bytes".to_string())?;
                let key = PrivateKey::try_new(bytes)
                    .map_err(|e| format!("invalid LEZ private key: {e}"))?;
                let public_key = PublicKey::new_from_private_key(&key);
                Ok(Self {
                    account_id: AccountId::from(&public_key),
                    key,
                })
            }
            LezAuth::Wallet { home, account_id } => {
                let wc = swap_orchestrator::scaffold::wallet_core(home)
                    .map_err(|e| format!("wallet home: {e}"))?;
                let key = wc
                    .get_account_public_signing_key(*account_id)
                    .ok_or_else(|| {
                        format!(
                            "wallet has no signing key for account {}",
                            account_id_to_base58(account_id)
                        )
                    })?
                    .clone();
                Ok(Self {
                    account_id: *account_id,
                    key,
                })
            }
        }
    }
}

pub struct Ctx {
    pub cfg: McpConfig,
    pub sequencer: SequencerClient,
    /// Present when a signing key is configured; wraps the app's LezClient.
    pub lez: Option<LezClient>,
    pub signer: Option<Signer>,
    gate: RwLock<GateState>,
    /// Serializes write-gate refreshes so only one live fingerprint runs at a
    /// time (single-flight). Without it, two concurrent refreshes race and a
    /// slower/older response can overwrite a newer verdict with a fresh stamp.
    refresh_lock: tokio::sync::Mutex<()>,
    /// Serializes the full faucet claim cycle (fetch challenge → solve → submit
    /// → confirm) across ALL invocations. The pinata seed rotates on every
    /// committed claim, so two concurrent claims could otherwise solve the same
    /// seed and have the loser's solution silently dropped/misattributed.
    /// `Arc` so an OWNED guard can move into the detached submit+confirm task
    /// (see [`run_guarded_detached`]) and survive an MCP request-drop.
    claim_lock: Arc<tokio::sync::Mutex<()>>,
    /// Serializes ALL signed writes (auth-transfer `Initialize` and `Transfer`)
    /// end-to-end — from the nonce read through commitment resolution. The
    /// sequencer nonce only advances when a transaction commits, and both
    /// `LezClient::transfer` and `initialize_signer_account` read the nonce
    /// independently, so two concurrent signed writes would otherwise read the
    /// same nonce and double-submit. There is exactly one signer per server
    /// today, so one global lock is the per-signer lock. Unsigned writes (the
    /// pinata claim submission itself) deliberately do NOT take it — only a
    /// claim's *auto-initialize* (which is signed) does.
    signed_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// Live transfer preview tokens, keyed by token string.
    preview_tokens: Mutex<HashMap<String, PreviewToken>>,
    /// Global guard limiting concurrent faucet PoW solves (CPU bound). With the
    /// per-target `claim_lock` above serializing whole cycles this is largely a
    /// secondary safety cap, but it keeps the solve-permit accounting honest.
    pow_permits: Arc<Semaphore>,
    eth: OnceCell<EthReader>,
}

impl Ctx {
    pub fn new(
        cfg: McpConfig,
        sequencer: SequencerClient,
        lez: Option<LezClient>,
        signer: Option<Signer>,
        gate: Gate,
    ) -> Self {
        Self {
            cfg,
            sequencer,
            lez,
            signer,
            gate: RwLock::new(GateState {
                gate,
                checked_at: Instant::now(),
            }),
            refresh_lock: tokio::sync::Mutex::new(()),
            claim_lock: Arc::new(tokio::sync::Mutex::new(())),
            signed_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            preview_tokens: Mutex::new(HashMap::new()),
            pow_permits: Arc::new(Semaphore::new(POW_CONCURRENCY)),
            eth: OnceCell::new(),
        }
    }

    async fn eth(&self) -> Result<&EthReader, String> {
        self.eth
            .get_or_try_init(|| {
                EthReader::connect(
                    &self.cfg.eth_rpc_url,
                    &self.cfg.eth_htlc_address,
                    self.cfg.eth_htlc_from_block,
                )
            })
            .await
    }

    /// Current cached write-gate verdict (no refresh).
    async fn writes_enabled(&self) -> bool {
        self.gate.read().await.gate.writes_enabled()
    }

    /// Gate a write. Any submission must be covered by a *fresh-enough* verified
    /// fingerprint: if the cached verification is stale (older than
    /// [`FINGERPRINT_TTL`]) or not currently verified, re-run getProgramIds
    /// against the configured sequencer before proceeding. A refresh failure —
    /// or a mismatch surfaced by the refresh — blocks the write. Called before
    /// EACH submission (every claim iteration, every transfer/initialize) so a
    /// mid-session testnet upgrade cannot let writes ride an obsolete
    /// fingerprint.
    async fn ensure_fresh_write_gate(&self) -> Result<(), CallToolResult> {
        // Fast path: currently verified and still fresh.
        {
            let gs = self.gate.read().await;
            if gate_is_fresh(&gs, FINGERPRINT_TTL, Instant::now()) {
                return Ok(());
            }
        }

        // Refresh against the configured sequencer — single-flight + double
        // checked so concurrent writers share one verification and cannot let
        // an older response resurrect a stale verdict.
        let gate = refresh_gate_single_flight(
            &self.gate,
            &self.refresh_lock,
            FINGERPRINT_TTL,
            || async {
                match run_fingerprint(&self.sequencer, &self.cfg.sequencer_url).await {
                    Ok(report) if report.matched => Gate::Verified(report),
                    Ok(report) => Gate::Mismatch(report),
                    Err(e) => Gate::Unverified { error: e },
                }
            },
        )
        .await;

        if gate.writes_enabled() {
            Ok(())
        } else {
            Err(err_json(json!({
                "ok": false,
                "error": "writes_disabled",
                "reason": "the live version fingerprint against the configured sequencer did not \
                           verify (re-checked immediately before this write); a mismatched \
                           client's transactions are silently dropped, so write tools are \
                           refused. Run lez_fingerprint (no arguments) for the full diff.",
                "fingerprint": gate.to_json(),
            })))
        }
    }

    /// Confirm a *specific* submitted transaction rather than trusting any
    /// balance credit (which concurrent transfers/claims or duplicate
    /// submissions can forge). Polls `getTransaction(tx_hash)`: the sequencer
    /// silently drops what it will not execute, so the tx appearing by hash is
    /// a tx-specific commitment signal. Falls back to a balance-delta check —
    /// explicitly marked weak — only if `getTransaction` is unavailable.
    /// Returns `(confirmed, strength)`.
    async fn confirm_submitted_tx(
        &self,
        tx_hash: &str,
        account: &AccountId,
        expected_min_balance: u128,
    ) -> (bool, &'static str) {
        if let Ok(hash) = tx_hash.parse::<HashType>() {
            let bytes = hash.0;
            let deadline = tokio::time::Instant::now() + COMMIT_TIMEOUT;
            loop {
                match self.sequencer.get_transaction(HashType(bytes)).await {
                    Ok(Some(_)) => return (true, "tx_committed"),
                    Ok(None) => {}
                    // RPC lacks/refused getTransaction — drop to the weak path.
                    Err(_) => break,
                }
                if tokio::time::Instant::now() >= deadline {
                    // Strong *negative*: the tx is not committed within the window.
                    return (false, "tx_committed");
                }
                tokio::time::sleep(COMMIT_POLL).await;
            }
        }

        // Weak fallback: any credit reaching the threshold (cross-confirmable).
        let (_bal, ok) = self.wait_balance_at_least(account, expected_min_balance).await;
        (ok, "balance_delta_weak")
    }

    async fn balance_of(&self, account_id: &AccountId) -> Result<u128, String> {
        // Thin wrapper over LezClient::get_balance when a client exists;
        // identical raw RPC otherwise (read-only mode).
        match &self.lez {
            Some(lez) => lez.get_balance(account_id).await.map_err(|e| e.to_string()),
            None => self
                .sequencer
                .get_account_balance(*account_id)
                .await
                .map_err(|e| format!("get_account_balance failed: {e}")),
        }
    }

    /// Wait until `account`'s balance reaches `min`. Returns the last
    /// observed balance and whether the target was reached.
    async fn wait_balance_at_least(&self, account: &AccountId, min: u128) -> (u128, bool) {
        let deadline = tokio::time::Instant::now() + COMMIT_TIMEOUT;
        loop {
            let balance = self.balance_of(account).await.unwrap_or(0);
            if balance >= min {
                return (balance, true);
            }
            if tokio::time::Instant::now() >= deadline {
                return (balance, false);
            }
            tokio::time::sleep(COMMIT_POLL).await;
        }
    }

    /// True when the account exists on-chain (has ever been initialized /
    /// claimed by a program). The sequencer SILENTLY DROPS pinata claims and
    /// transfers that reference never-initialized accounts, so tools must
    /// check this up front (mirrors the wallet CLI's
    /// `ensure_public_recipient_initialized`).
    async fn account_exists(&self, account_id: &AccountId) -> Result<bool, String> {
        let account = self
            .sequencer
            .get_account(*account_id)
            .await
            .map_err(|e| format!("get_account failed: {e}"))?;
        Ok(account != lee_core::account::Account::default())
    }

    /// Send `authenticated_transfer::Initialize` for the signer's account and
    /// wait until the account is owned by the auth-transfer program.
    ///
    /// SIGNED write: callers must hold [`Ctx::signed_write_lock`] across this
    /// call — it reads the account nonce independently, so running it
    /// concurrently with any other signed write races the nonce.
    async fn initialize_signer_account(&self, signer: &Signer) -> Result<String, String> {
        let program_id = programs::authenticated_transfer().id();
        let nonces = self
            .sequencer
            .get_accounts_nonces(vec![signer.account_id])
            .await
            .map_err(|e| format!("get_accounts_nonces failed: {e}"))?;

        let message = lee::public_transaction::Message::try_new(
            program_id,
            vec![signer.account_id],
            nonces,
            authenticated_transfer_core::Instruction::Initialize,
        )
        .map_err(|e| format!("failed to build Initialize message: {e}"))?;
        let witness_set = WitnessSet::for_message(&message, &[&signer.key]);
        let tx = PublicTransaction::new(message, witness_set);

        let tx_hash = self
            .sequencer
            .send_transaction(LeeTransaction::Public(tx))
            .await
            .map_err(|e| format!("Initialize submission failed: {e}"))?
            .to_string();

        // Wait for the account to become auth-transfer-owned.
        let deadline = tokio::time::Instant::now() + COMMIT_TIMEOUT;
        loop {
            let account = self
                .sequencer
                .get_account(signer.account_id)
                .await
                .map_err(|e| format!("get_account failed: {e}"))?;
            if account.program_owner == program_id {
                return Ok(tx_hash);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "account Initialize did not commit within {}s (tx {tx_hash})",
                    COMMIT_TIMEOUT.as_secs()
                ));
            }
            tokio::time::sleep(COMMIT_POLL).await;
        }
    }
}

/// Run `work` inside a SPAWNED task that owns `guard`, and await its result.
///
/// This is the drop-survival primitive for write critical sections: once the
/// submission side of a write has started, the section must run to commitment
/// resolution even if the MCP request future is dropped (client cancellation /
/// disconnect). A plain `.await` in the request would drop the guard
/// mid-broadcast — the tx can still commit afterwards, so the next caller
/// would race it (unrotated pinata seed, unadvanced nonce). Spawning detaches
/// the critical section from the request: on request-drop the task keeps
/// running and releases the lock only after `work` (submit + confirm) has
/// resolved. The request merely awaits the JoinHandle; a panic inside the task
/// is surfaced as an error.
async fn run_guarded_detached<G, F, T>(guard: G, work: F) -> Result<T, String>
where
    G: Send + 'static,
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = tokio::spawn(async move {
        // The guard lives HERE — released when the critical section genuinely
        // finishes, not when the awaiting request future is dropped.
        let _guard = guard;
        work.await
    });
    handle
        .await
        .map_err(|e| format!("write critical-section task failed: {e}"))
}

// ── Result helpers ──────────────────────────────────────────────────────

/// Success result carrying both structured JSON and its text rendering.
fn ok_json(v: Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
    let mut r = CallToolResult::success(vec![ContentBlock::text(text)]);
    r.structured_content = Some(v);
    r
}

/// Tool-level error (the tool ran; the caller should see why it failed).
fn err_json(v: Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
    let mut r = CallToolResult::error(vec![ContentBlock::text(text)]);
    r.structured_content = Some(v);
    r
}

fn tool_fail(message: impl Into<String>) -> CallToolResult {
    err_json(json!({"ok": false, "error": message.into()}))
}

/// A fresh, unguessable preview token (256 bits from the system CSPRNG).
fn new_preview_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("system CSPRNG unavailable");
    hex::encode(buf)
}

/// Drop preview tokens expired as of `now` so the store cannot grow unbounded.
fn prune_preview_tokens_at(store: &mut HashMap<String, PreviewToken>, now: Instant) {
    store.retain(|_, t| t.expires > now);
}

/// Drop expired preview tokens (relative to the current instant).
fn prune_preview_tokens(store: &mut HashMap<String, PreviewToken>) {
    prune_preview_tokens_at(store, Instant::now());
}

#[derive(Debug, PartialEq, Eq)]
enum TokenCheck {
    Ok,
    /// Unknown, already-consumed, or pruned-as-expired token.
    Missing,
    /// Present but past its expiry.
    Expired,
    /// Present and live, but bound to different (from, to, amount).
    Mismatch,
}

/// Validate a preview token against the intended transfer parameters and
/// consume it (single-use — removed whether or not it validates). Prunes
/// expired tokens first so the store cannot accumulate stale entries.
fn check_and_consume_token(
    store: &mut HashMap<String, PreviewToken>,
    token: &str,
    from: &AccountId,
    to: &AccountId,
    amount: u128,
    now: Instant,
) -> TokenCheck {
    prune_preview_tokens_at(store, now);
    match store.remove(token) {
        None => TokenCheck::Missing,
        Some(pt) => {
            if pt.expires <= now {
                TokenCheck::Expired
            } else if &pt.from != from || &pt.to != to || pt.amount != amount {
                TokenCheck::Mismatch
            } else {
                TokenCheck::Ok
            }
        }
    }
}

// ── Tool parameters ─────────────────────────────────────────────────────

#[derive(Deserialize, schemars::JsonSchema)]
pub struct BalanceParams {
    /// Base58 LEZ account id. Omit to read the server's own account
    /// (requires a configured signing key).
    pub account_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct FingerprintParams {
    /// Sequencer RPC URL to fingerprint. Omit to check the configured
    /// sequencer (which also refreshes the write gate).
    pub sequencer_url: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct FaucetParams {
    /// Base58 LEZ account id to credit (150 LEZ per claim).
    pub account_id: String,
    /// Claim repeatedly until the balance reaches this value (u128, as a
    /// string). Omit for a single claim.
    pub target_balance: Option<String>,
    /// Safety cap on claim iterations (default 10, max 20).
    pub max_claims: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct TransferParams {
    /// Base58 recipient account id.
    pub to: String,
    /// Amount in LEZ base units (u128, as a string).
    pub amount: String,
    /// false (default) returns a dry-run preview; true broadcasts.
    #[serde(default)]
    pub confirm: bool,
    /// Server-issued token from a prior confirm=false preview. REQUIRED when
    /// confirm=true: it binds this broadcast to the exact previewed
    /// (from, to, amount), is single-use, and expires after a few minutes.
    pub preview_token: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct HtlcStatusParams {
    /// 32-byte hex (0x-optional): an EthHTLC swap id, or a hashlock to look
    /// up via Locked-event scan.
    pub swap_id_or_hashlock: String,
}

// ── The server ──────────────────────────────────────────────────────────

pub struct LezMcpServer {
    ctx: Arc<Ctx>,
}

impl LezMcpServer {
    pub fn new(ctx: Arc<Ctx>) -> Self {
        Self { ctx }
    }
}

#[tool_router]
impl LezMcpServer {
    #[tool(
        name = "lez_balance",
        description = "Read the LEZ balance of an account on the configured sequencer. \
                       Returns the balance in LEZ base units as a string."
    )]
    async fn lez_balance(
        &self,
        Parameters(p): Parameters<BalanceParams>,
    ) -> Result<CallToolResult, McpError> {
        let account_id = match &p.account_id {
            Some(s) => match parse_base58_account_id(s) {
                Ok(id) => id,
                Err(e) => return Ok(tool_fail(format!("invalid account_id: {e}"))),
            },
            None => match &self.ctx.signer {
                Some(signer) => signer.account_id,
                None => {
                    return Ok(tool_fail(
                        "no account_id given and no signing key configured — pass account_id \
                         or set LEZ_SIGNING_KEY / LEZ_WALLET_HOME+LEZ_ACCOUNT_ID",
                    ));
                }
            },
        };

        match self.ctx.balance_of(&account_id).await {
            Ok(balance) => Ok(ok_json(json!({
                "ok": true,
                "account_id": account_id_to_base58(&account_id),
                "balance": balance.to_string(),
                "sequencer_url": redact_url(&self.ctx.cfg.sequencer_url),
            }))),
            Err(e) => Ok(tool_fail(e)),
        }
    }

    #[tool(
        name = "lez_fingerprint",
        description = "Compare the sequencer's builtin program ImageIDs (getProgramIds RPC) \
                       against the LEZ v0.2.0 ImageIDs embedded in this server. Reports a \
                       match/mismatch verdict per program. Without arguments it checks the \
                       configured sequencer and refreshes the write gate."
    )]
    async fn lez_fingerprint(
        &self,
        Parameters(p): Parameters<FingerprintParams>,
    ) -> Result<CallToolResult, McpError> {
        let (client, url, updates_gate) = match &p.sequencer_url {
            Some(url) => {
                let parsed = match url::Url::parse(url) {
                    Ok(u) => u,
                    Err(e) => return Ok(tool_fail(format!("invalid sequencer_url: {e}"))),
                };
                let client = match SequencerClientBuilder::default().build(parsed) {
                    Ok(c) => c,
                    Err(e) => return Ok(tool_fail(format!("failed to create client: {e}"))),
                };
                let updates_gate = url.trim_end_matches('/')
                    == self.ctx.cfg.sequencer_url.trim_end_matches('/');
                (client, url.clone(), updates_gate)
            }
            None => (
                self.ctx.sequencer.clone(),
                self.ctx.cfg.sequencer_url.clone(),
                true,
            ),
        };

        // When this re-check targets the configured sequencer it updates the
        // gate. Serialize that run+store under the same lock the write-gate
        // refresh uses, so an explicit re-check and a concurrent write-path
        // refresh cannot interleave and resurrect a stale verdict.
        let outcome = if updates_gate {
            let _flight = self.ctx.refresh_lock.lock().await;
            let outcome = run_fingerprint(&client, &url).await;
            let mut gs = self.ctx.gate.write().await;
            gs.gate = match &outcome {
                Ok(report) if report.matched => Gate::Verified(report.clone()),
                Ok(report) => Gate::Mismatch(report.clone()),
                Err(e) => Gate::Unverified { error: e.clone() },
            };
            gs.checked_at = Instant::now();
            outcome
        } else {
            run_fingerprint(&client, &url).await
        };

        match outcome {
            Ok(report) => {
                let verdict = if report.matched { "match" } else { "mismatch" };
                Ok(ok_json(json!({
                    "ok": true,
                    "verdict": verdict,
                    "report": report,
                    "writes_enabled": self.ctx.writes_enabled().await,
                })))
            }
            Err(e) => Ok(err_json(json!({
                "ok": false,
                "error": e,
                "writes_enabled": self.ctx.writes_enabled().await,
            }))),
        }
    }

    #[tool(
        name = "lez_faucet_claim",
        description = "Claim testnet LEZ from the pinata faucet (150 LEZ per claim, \
                       proof-of-work solved locally, no signature needed). Optionally claim \
                       repeatedly until the account balance reaches target_balance. Each claim \
                       waits for on-chain commitment (~30-60s per block), so multi-claim calls \
                       can take minutes."
    )]
    async fn lez_faucet_claim(
        &self,
        Parameters(p): Parameters<FaucetParams>,
    ) -> Result<CallToolResult, McpError> {
        // Fresh write gate before any submission (init included); re-checked
        // per claim inside the loop below.
        if let Err(refusal) = self.ctx.ensure_fresh_write_gate().await {
            return Ok(refusal);
        }

        let account_id = match parse_base58_account_id(&p.account_id) {
            Ok(id) => id,
            Err(e) => return Ok(tool_fail(format!("invalid account_id: {e}"))),
        };
        let target: Option<u128> = match &p.target_balance {
            Some(s) => match s.parse() {
                Ok(v) => Some(v),
                Err(e) => return Ok(tool_fail(format!("invalid target_balance: {e}"))),
            },
            None => None,
        };
        let max_claims = p.max_claims.unwrap_or(10).clamp(1, MAX_FAUCET_CLAIMS);

        // The sequencer silently drops claims to never-initialized accounts.
        // Auto-initialize when we hold the account's signing key; otherwise
        // explain what is needed (mirrors `wallet auth-transfer init`).
        let mut init_tx_hash = None;
        match self.ctx.account_exists(&account_id).await {
            Err(e) => return Ok(tool_fail(e)),
            Ok(true) => {}
            Ok(false) => match &self.ctx.signer {
                Some(signer) if signer.account_id == account_id => {
                    // Auto-initialize is a SIGNED write: serialize it against
                    // every other signed write ([P1-1] — it reads the nonce
                    // independently), and run it detached so an MCP
                    // request-drop cannot release the lock while the
                    // Initialize may still commit.
                    let guard = self.ctx.signed_write_lock.clone().lock_owned().await;
                    let ctx = self.ctx.clone();
                    let outcome = run_guarded_detached(guard, async move {
                        let signer = ctx.signer.as_ref().expect("signer checked above");
                        ctx.initialize_signer_account(signer).await
                    })
                    .await;
                    match outcome {
                        Ok(Ok(h)) => init_tx_hash = Some(h),
                        Ok(Err(e)) | Err(e) => {
                            return Ok(tool_fail(format!(
                                "account initialization failed: {e}"
                            )));
                        }
                    }
                }
                _ => {
                    return Ok(err_json(json!({
                        "ok": false,
                        "error": "account_uninitialized",
                        "reason": format!(
                            "account {} has never been initialized on-chain; the sequencer \
                             silently drops faucet claims to such accounts. Initialize it \
                             first (`wallet auth-transfer init`), or configure this server \
                             with the account's signing key (LEZ_SIGNING_KEY) so it can \
                             initialize automatically.",
                            account_id_to_base58(&account_id)
                        ),
                    })));
                }
            },
        }

        let start_balance = match self.ctx.balance_of(&account_id).await {
            Ok(b) => b,
            Err(e) => return Ok(tool_fail(e)),
        };

        let mut balance = start_balance;
        let mut claims = Vec::new();
        let mut incomplete: Option<String> = None;

        for _ in 0..max_claims {
            if let Some(t) = target
                && balance >= t
            {
                break;
            }

            // Serialize the ENTIRE claim cycle (fetch challenge → solve → submit
            // → confirm) across all invocations: the pinata seed rotates on each
            // committed claim, so overlapping cycles could otherwise solve the
            // same seed and have the loser silently dropped. Owned guard: once
            // submission starts it moves into a detached task ([P2-2] below) and
            // is released only after this iteration's commitment resolves.
            let claim_guard = self.ctx.claim_lock.clone().lock_owned().await;

            // [P2-3] Re-evaluate the target decision UNDER the lock. A request
            // queued behind another claim cycle sees a pre-lock balance that is
            // stale by the time the lock is granted; deciding on it would fire
            // an unnecessary extra claim. Re-read before fetching the challenge
            // (keep the last-seen balance on a transient read error — the
            // per-claim confirm threshold below stays conservative).
            balance = self.ctx.balance_of(&account_id).await.unwrap_or(balance);
            if let Some(t) = target
                && balance >= t
            {
                break; // guard drops here; nothing was submitted
            }

            // Solve first (fetch challenge + PoW; may take up to ~45s and may
            // wait on the PoW semaphore). The gate is (re)checked AFTER this,
            // immediately before the send, so a long solve/wait cannot let the
            // submission ride a fingerprint that has since gone stale.
            let solved = match faucet::solve_claim(
                &self.ctx.sequencer,
                account_id,
                self.ctx.pow_permits.clone(),
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    incomplete = Some(e);
                    break;
                }
            };

            // Fresh write gate immediately before the send (post-PoW,
            // post-semaphore). A failed recheck aborts THIS submission.
            if let Err(refusal) = self.ctx.ensure_fresh_write_gate().await {
                return Ok(refusal);
            }

            // [P2-2] From here the claim may reach the sequencer. Run submit +
            // confirm in a detached task that OWNS the claim guard: if the MCP
            // request is cancelled mid-broadcast, the task still runs to
            // commitment resolution and only then releases the lock — the next
            // claim can never solve against a seed an in-flight commit is about
            // to rotate.
            let ctx = self.ctx.clone();
            let expected_min = balance + faucet::PRIZE_PER_CLAIM;
            let outcome = run_guarded_detached(claim_guard, async move {
                let submission = faucet::submit_solved_claim(&ctx.sequencer, &solved).await?;
                let (confirmed, confirmation) = ctx
                    .confirm_submitted_tx(&submission.tx_hash, &account_id, expected_min)
                    .await;
                Ok::<_, String>((submission, confirmed, confirmation))
            })
            .await;
            let (submission, confirmed, confirmation) = match outcome {
                Ok(Ok(v)) => v,
                Ok(Err(e)) | Err(e) => {
                    incomplete = Some(e);
                    break;
                }
            };

            let new_balance = self.ctx.balance_of(&account_id).await.unwrap_or(balance);
            claims.push(json!({
                "tx_hash": submission.tx_hash,
                "pow_solution": submission.solution.to_string(),
                "confirmed": confirmed,
                "confirmation": confirmation,
                "balance_after": new_balance.to_string(),
            }));
            balance = new_balance;

            if !confirmed {
                incomplete = Some(format!(
                    "claim {} not confirmed within {}s — it may still land; stopping",
                    submission.tx_hash,
                    COMMIT_TIMEOUT.as_secs()
                ));
                break;
            }
            if target.is_none() {
                break;
            }
        }

        let target_reached = target.map(|t| balance >= t);
        Ok(ok_json(json!({
            "ok": incomplete.is_none(),
            "init_tx_hash": init_tx_hash,
            "account_id": account_id_to_base58(&account_id),
            "prize_per_claim": faucet::PRIZE_PER_CLAIM.to_string(),
            "claims_made": claims.len(),
            "claims": claims,
            "start_balance": start_balance.to_string(),
            "final_balance": balance.to_string(),
            "target_balance": target.map(|t| t.to_string()),
            "target_reached": target_reached,
            "warning": incomplete,
        })))
    }

    #[tool(
        name = "lez_transfer",
        description = "Transfer LEZ (authenticated transfer, feeless). TWO-PHASE: with \
                       confirm=false (default) nothing is sent — you get a dry-run preview with \
                       current balances; only confirm=true broadcasts. Refused when the startup \
                       version fingerprint did not match. Amounts are u128 base units as strings."
    )]
    async fn lez_transfer(
        &self,
        Parameters(p): Parameters<TransferParams>,
    ) -> Result<CallToolResult, McpError> {
        let (Some(_lez), Some(signer)) = (&self.ctx.lez, &self.ctx.signer) else {
            return Ok(tool_fail(
                "no signing key configured — set LEZ_SIGNING_KEY, LEZ_SIGNING_KEY_FILE, or \
                 LEZ_WALLET_HOME + LEZ_ACCOUNT_ID in the server environment",
            ));
        };

        let to = match parse_base58_account_id(&p.to) {
            Ok(id) => id,
            Err(e) => return Ok(tool_fail(format!("invalid to: {e}"))),
        };
        let amount: u128 = match p.amount.parse() {
            Ok(v) if v > 0 => v,
            Ok(_) => return Ok(tool_fail("amount must be > 0")),
            Err(e) => return Ok(tool_fail(format!("invalid amount: {e}"))),
        };

        let from = signer.account_id;
        let from_account = match self.ctx.sequencer.get_account(from).await {
            Ok(a) => a,
            Err(e) => return Ok(tool_fail(format!("get_account(from) failed: {e}"))),
        };
        let to_account = match self.ctx.sequencer.get_account(to).await {
            Ok(a) => a,
            Err(e) => return Ok(tool_fail(format!("get_account(to) failed: {e}"))),
        };
        let to_balance = to_account.balance;
        let recipient_initialized = to_account != lee_core::account::Account::default();

        let auth_transfer_id = programs::authenticated_transfer().id();
        let sender_initialized = from_account.program_owner == auth_transfer_id;
        let sufficient = from_account.balance >= amount;
        let recipient_warning = (!recipient_initialized).then_some(
            "recipient account has never been initialized on-chain — the sequencer may \
             silently drop this transfer; the recipient should run `wallet auth-transfer \
             init` (or claim from the faucet via a lez-mcp server holding their key) first",
        );

        if !p.confirm {
            // Issue a single-use preview token bound to these exact parameters;
            // confirm=true will require it. Prevents a caller-flipped
            // confirm=true from broadcasting anything never previewed.
            let token = new_preview_token();
            {
                let mut store = self.ctx.preview_tokens.lock().unwrap();
                prune_preview_tokens(&mut store);
                store.insert(
                    token.clone(),
                    PreviewToken {
                        from,
                        to,
                        amount,
                        expires: Instant::now() + PREVIEW_TTL,
                    },
                );
            }
            return Ok(ok_json(json!({
                "ok": true,
                "dry_run": true,
                "action": "lez_transfer",
                "from": account_id_to_base58(&from),
                "to": account_id_to_base58(&to),
                "amount": amount.to_string(),
                "fee": "0 (LEZ transfers are feeless)",
                "from_balance": from_account.balance.to_string(),
                "to_balance": to_balance.to_string(),
                "from_nonce": serde_json::to_value(from_account.nonce).unwrap_or(Value::Null),
                "sender_initialized": sender_initialized,
                "will_initialize_first": !sender_initialized,
                "recipient_initialized": recipient_initialized,
                "recipient_warning": recipient_warning,
                "sufficient_balance": sufficient,
                "writes_enabled": self.ctx.writes_enabled().await,
                "preview_token": token,
                "preview_expires_in_secs": PREVIEW_TTL.as_secs(),
                "note": "nothing was sent — call again with confirm=true AND preview_token to \
                         broadcast (the token binds to this exact from/to/amount, is single-use, \
                         and expires)",
            })));
        }

        // confirm=true — the write path.

        // 1) Require and consume a matching, unexpired preview token. Single-use:
        //    removed on lookup so it cannot be replayed.
        let token = match &p.preview_token {
            Some(t) => t.clone(),
            None => {
                return Ok(tool_fail(
                    "confirm=true requires preview_token — call lez_transfer with confirm=false \
                     first to preview and obtain a token",
                ));
            }
        };
        let check = {
            let mut store = self.ctx.preview_tokens.lock().unwrap();
            check_and_consume_token(&mut store, &token, &from, &to, amount, Instant::now())
        };
        match check {
            TokenCheck::Ok => {}
            TokenCheck::Missing => {
                return Ok(tool_fail(
                    "preview_token is unknown, already used, or expired — run a fresh \
                     confirm=false preview",
                ));
            }
            TokenCheck::Expired => {
                return Ok(tool_fail(
                    "preview_token expired — run a fresh confirm=false preview",
                ));
            }
            TokenCheck::Mismatch => {
                return Ok(tool_fail(
                    "preview_token does not match this (from, to, amount) — run a fresh \
                     confirm=false preview for these exact parameters",
                ));
            }
        }

        // 2) Fresh write gate (early refusal before queueing on the write lock;
        //    re-verified again inside the critical section below).
        if let Err(refusal) = self.ctx.ensure_fresh_write_gate().await {
            return Ok(refusal);
        }

        // 3) Refuse a transfer to a never-initialized recipient — the sequencer
        //    silently drops it (a known silent-drop), so do not broadcast + poll.
        if !recipient_initialized {
            return Ok(err_json(json!({
                "ok": false,
                "error": "recipient_uninitialized",
                "reason": format!(
                    "recipient {} has never been initialized on-chain; the sequencer silently \
                     drops authenticated transfers to such accounts, so this write is refused. \
                     The recipient must initialize first (`wallet auth-transfer init`) or receive \
                     a faucet claim via a lez-mcp server holding their key.",
                    account_id_to_base58(&to)
                ),
            })));
        }

        // 4) Only auto-initialize a genuinely-default (unowned) sender. An
        //    account owned by another program cannot be Initialized and its
        //    Transfer is guaranteed to fail — report that immediately instead
        //    of a doomed Initialize + 4-minute poll.
        let sender_default_owned = from_account.program_owner == [0u32; 8];
        if !sender_initialized && !sender_default_owned {
            return Ok(err_json(json!({
                "ok": false,
                "error": "sender_incompatible_ownership",
                "reason": format!(
                    "sender {} is owned by program {} — not the authenticated-transfer program \
                     and not an uninitialized account. It cannot be auto-initialized and its \
                     Transfer would be rejected. Use an authenticated-transfer account.",
                    account_id_to_base58(&from),
                    crate::fingerprint::program_id_hex(&from_account.program_owner)
                ),
            })));
        }

        if !sufficient {
            return Ok(tool_fail(format!(
                "insufficient balance: from has {} < amount {}",
                from_account.balance, amount
            )));
        }

        // 5) [P1-1] The signed critical section: (maybe) Initialize, then
        //    Transfer, then confirm — all under the global signed-write lock,
        //    held from BEFORE the first nonce read until commitment resolution.
        //    Both Initialize and LezClient::transfer read the signer's nonce
        //    independently, and the nonce only advances when a tx commits, so
        //    two concurrent confirmed transfers (or a transfer racing a faucet
        //    auto-init) would otherwise read the same nonce and double-submit.
        //    Run detached ([`run_guarded_detached`]): an MCP request-drop
        //    mid-broadcast cannot release the lock while the tx may still
        //    commit with the old nonce.
        let signed_guard = self.ctx.signed_write_lock.clone().lock_owned().await;
        let ctx = self.ctx.clone();
        let sender_needs_init = !sender_initialized;
        let outcome = run_guarded_detached(signed_guard, async move {
            let signer = ctx.signer.as_ref().expect("signer checked above");
            let lez = ctx.lez.as_ref().expect("lez client checked above");

            // Waiting on the write lock can outlast the fingerprint TTL;
            // re-verify before the first signed submission.
            ctx.ensure_fresh_write_gate().await?;

            // A fresh raw-key account is not yet owned by the auth-transfer
            // program; the sequencer would reject its Transfer. Initialize
            // first (same as `wallet auth-transfer init`).
            let init_tx_hash = if sender_needs_init {
                match ctx.initialize_signer_account(signer).await {
                    Ok(h) => Some(h),
                    Err(e) => {
                        return Err(tool_fail(format!("account initialization failed: {e}")));
                    }
                }
            } else {
                None
            };

            // Initialize can take minutes; re-verify the gate is still fresh
            // before the actual Transfer submission.
            ctx.ensure_fresh_write_gate().await?;

            let tx_hash = match lez.transfer(to, amount).await {
                Ok(h) => h,
                Err(e) => return Err(tool_fail(format!("transfer failed: {e}"))),
            };

            // Confirm THIS transaction committed (tx-specific), not merely that
            // the recipient balance moved (which concurrent activity could
            // forge). Still inside the critical section: the nonce advances
            // only on commitment, so the next signed write must wait for it.
            let (confirmed, confirmation) = ctx
                .confirm_submitted_tx(&tx_hash, &to, to_balance + amount)
                .await;
            let final_to_balance = ctx.balance_of(&to).await.unwrap_or(to_balance);
            Ok((init_tx_hash, tx_hash, confirmed, confirmation, final_to_balance))
        })
        .await;
        let (init_tx_hash, tx_hash, confirmed, confirmation, final_to_balance) = match outcome {
            Ok(Ok(v)) => v,
            Ok(Err(refusal)) => return Ok(refusal),
            Err(e) => return Ok(tool_fail(e)),
        };

        Ok(ok_json(json!({
            "ok": true,
            "dry_run": false,
            "tx_hash": tx_hash,
            "init_tx_hash": init_tx_hash,
            "from": account_id_to_base58(&from),
            "to": account_id_to_base58(&to),
            "amount": amount.to_string(),
            "confirmed": confirmed,
            "confirmation": confirmation,
            "to_balance_after": final_to_balance.to_string(),
            "warning": (!confirmed).then(|| format!(
                "transaction {tx_hash} was not confirmed committed within {}s — it may still land \
                 or may have been rejected at execution; re-check with sepolia/lez tools",
                COMMIT_TIMEOUT.as_secs()
            )),
        })))
    }

    #[tool(
        name = "sepolia_htlc_status",
        description = "Read the state of an HTLC on the deployed Sepolia EthHTLC contract \
                       (read-only, no key needed). Accepts a swap id, or a hashlock which is \
                       resolved by scanning Locked events."
    )]
    async fn sepolia_htlc_status(
        &self,
        Parameters(p): Parameters<HtlcStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let input = match parse_bytes32(&p.swap_id_or_hashlock) {
            Ok(b) => b,
            Err(e) => return Ok(tool_fail(format!("invalid swap_id_or_hashlock: {e}"))),
        };

        let eth = match self.ctx.eth().await {
            Ok(e) => e,
            Err(e) => return Ok(tool_fail(e)),
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let htlc_json = |swap_id: &[u8; 32],
                         h: &swap_orchestrator::eth::client::EthHTLC::HTLC|
         -> Value {
            let timelock = h.timelock.to::<u128>() as u64;
            json!({
                "swap_id": format!("0x{}", hex::encode(swap_id)),
                "state": state_str(&h.state),
                "sender": h.sender.to_string(),
                "recipient": h.recipient.to_string(),
                "amount_wei": h.amount.to_string(),
                "amount_eth": swap_orchestrator::config::wei_to_eth_string(
                    h.amount.to::<u128>()
                ),
                "hashlock": format!("0x{}", hex::encode(h.hashlock.0)),
                "timelock": timelock,
                "refundable_in_secs": timelock.saturating_sub(now),
            })
        };

        // First interpretation: the input is a swap id.
        match eth.htlc_by_swap_id(input).await {
            Ok(h) if state_str(&h.state) != "EMPTY" => {
                return Ok(ok_json(json!({
                    "ok": true,
                    "found": true,
                    "resolved_as": "swap_id",
                    "contract": eth.address().to_string(),
                    "htlcs": [htlc_json(&input, &h)],
                })));
            }
            Ok(_) => {}
            Err(e) => return Ok(tool_fail(e)),
        }

        // Fallback: treat it as a hashlock and scan Locked events (bounded +
        // incremental; a repeated call resumes rather than rescans).
        match eth.find_by_hashlock(input).await {
            Ok(scan) => {
                let htlcs: Vec<Value> = scan
                    .found
                    .iter()
                    .map(|f| htlc_json(&f.swap_id, &f.htlc))
                    .collect();
                let resolved_as = if !htlcs.is_empty() {
                    "hashlock"
                } else if scan.reached_head {
                    "not_found"
                } else {
                    "not_found_within_budget"
                };
                let mut out = json!({
                    "ok": true,
                    "found": !htlcs.is_empty(),
                    "resolved_as": resolved_as,
                    "contract": eth.address().to_string(),
                    "scanned_blocks": {
                        "from": scan.coverage_from,
                        "to": scan.coverage_to,
                        "reached_head": scan.reached_head,
                    },
                    "htlcs": htlcs,
                });
                if htlcs.is_empty() && !scan.reached_head {
                    out["note"] = json!(format!(
                        "not found within the {} blocks scanned so far (up to block {}); the scan \
                         budget was reached before chain head — call again to continue from there",
                        scan.coverage_to.saturating_sub(scan.coverage_from),
                        scan.coverage_to
                    ));
                }
                Ok(ok_json(out))
            }
            Err(e) => Ok(tool_fail(e)),
        }
    }
}

#[tool_handler]
impl ServerHandler for LezMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        let mut implementation = Implementation::default();
        implementation.name = "lez-mcp".into();
        implementation.version = env!("CARGO_PKG_VERSION").into();
        info.server_info = implementation;
        info.instructions = Some(format!(
                "LEZ public-testnet tools (sequencer: {}) plus the Sepolia EthHTLC (contract \
                 {}). Reads: lez_balance, lez_fingerprint, sepolia_htlc_status. Writes: \
                 lez_faucet_claim, lez_transfer — both are refused unless the startup version \
                 fingerprint (embedded LEZ {} ImageIDs vs the sequencer's getProgramIds) \
                 matched; run lez_fingerprint to re-check. lez_transfer is two-phase: \
                 confirm=false previews, confirm=true broadcasts. All amounts are LEZ base \
                 units (u128) passed as strings.",
                redact_url(&self.ctx.cfg.sequencer_url),
                self.ctx.cfg.eth_htlc_address,
                crate::fingerprint::CLIENT_TAG,
        ));
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_of(name: &str) -> Value {
        let tools = LezMcpServer::tool_router().list_all();
        let tool = tools
            .into_iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("tool {name} not advertised"));
        serde_json::to_value(tool.input_schema).expect("schema serializes")
    }

    #[test]
    fn advertises_exactly_the_five_tools() {
        let mut names: Vec<String> = LezMcpServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "lez_balance",
                "lez_faucet_claim",
                "lez_fingerprint",
                "lez_transfer",
                "sepolia_htlc_status",
            ]
        );
    }

    fn acct(byte: u8) -> AccountId {
        AccountId::new([byte; 32])
    }

    fn insert_token(
        store: &mut HashMap<String, PreviewToken>,
        token: &str,
        from: AccountId,
        to: AccountId,
        amount: u128,
        expires: Instant,
    ) {
        store.insert(
            token.to_string(),
            PreviewToken {
                from,
                to,
                amount,
                expires,
            },
        );
    }

    #[test]
    fn preview_token_is_random_and_32_bytes() {
        let a = new_preview_token();
        let b = new_preview_token();
        assert_eq!(a.len(), 64, "256-bit hex");
        assert!(hex::decode(&a).is_ok());
        assert_ne!(a, b, "tokens must not repeat");
    }

    #[test]
    fn token_flow_happy_then_replay_is_rejected() {
        let mut store = HashMap::new();
        let (from, to, amount) = (acct(1), acct(2), 500u128);
        let now = Instant::now();
        insert_token(&mut store, "tok", from, to, amount, now + PREVIEW_TTL);

        // Happy path consumes the token.
        assert_eq!(
            check_and_consume_token(&mut store, "tok", &from, &to, amount, now),
            TokenCheck::Ok
        );
        // Replay: single-use, now gone.
        assert_eq!(
            check_and_consume_token(&mut store, "tok", &from, &to, amount, now),
            TokenCheck::Missing
        );
        assert!(store.is_empty());
    }

    #[test]
    fn token_flow_rejects_mismatched_params() {
        let mut store = HashMap::new();
        let (from, to, amount) = (acct(1), acct(2), 500u128);
        let now = Instant::now();

        insert_token(&mut store, "amt", from, to, amount, now + PREVIEW_TTL);
        assert_eq!(
            check_and_consume_token(&mut store, "amt", &from, &to, 501, now),
            TokenCheck::Mismatch
        );

        insert_token(&mut store, "rcp", from, to, amount, now + PREVIEW_TTL);
        assert_eq!(
            check_and_consume_token(&mut store, "rcp", &from, &acct(9), amount, now),
            TokenCheck::Mismatch
        );

        insert_token(&mut store, "snd", from, to, amount, now + PREVIEW_TTL);
        assert_eq!(
            check_and_consume_token(&mut store, "snd", &acct(9), &to, amount, now),
            TokenCheck::Mismatch
        );
    }

    #[test]
    fn token_flow_rejects_expired_and_prunes() {
        let mut store = HashMap::new();
        let (from, to, amount) = (acct(1), acct(2), 500u128);
        let now = Instant::now();
        // Expired 1s ago.
        insert_token(&mut store, "old", from, to, amount, now - Duration::from_secs(1));
        // check_and_consume prunes expired first, so an expired token reads as
        // Missing (pruned) rather than lingering.
        assert_eq!(
            check_and_consume_token(&mut store, "old", &from, &to, amount, now),
            TokenCheck::Missing
        );
        assert!(store.is_empty(), "expired token must be pruned");
    }

    #[test]
    fn token_flow_unknown_token_is_missing() {
        let mut store = HashMap::new();
        assert_eq!(
            check_and_consume_token(&mut store, "nope", &acct(1), &acct(2), 1, Instant::now()),
            TokenCheck::Missing
        );
    }

    #[test]
    fn fingerprint_ttl_gate_freshness() {
        let embedded = crate::fingerprint::embedded_program_ids();
        let verified = Gate::Verified(crate::fingerprint::build_report(
            "http://x",
            &embedded,
            &embedded.clone(),
        ));
        let now = Instant::now();

        // Verified + fresh → may ride the cache.
        let fresh = GateState {
            gate: verified.clone(),
            checked_at: now,
        };
        assert!(gate_is_fresh(&fresh, FINGERPRINT_TTL, now));

        // Verified but stale → must refresh.
        let stale = GateState {
            gate: verified,
            checked_at: now - FINGERPRINT_TTL - Duration::from_secs(1),
        };
        assert!(!gate_is_fresh(&stale, FINGERPRINT_TTL, now));

        // Unverified, even if just checked → never fresh.
        let unverified = GateState {
            gate: Gate::Unverified { error: "down".into() },
            checked_at: now,
        };
        assert!(!gate_is_fresh(&unverified, FINGERPRINT_TTL, now));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn single_flight_refresh_runs_once_under_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let embedded = crate::fingerprint::embedded_program_ids();
        let verified = Gate::Verified(crate::fingerprint::build_report(
            "http://x",
            &embedded,
            &embedded.clone(),
        ));

        // Start stale so the first caller must refresh.
        let gate = Arc::new(RwLock::new(GateState {
            gate: Gate::Unverified { error: "stale".into() },
            checked_at: Instant::now() - FINGERPRINT_TTL - Duration::from_secs(1),
        }));
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let runs = Arc::new(AtomicUsize::new(0));

        // Fire many concurrent refreshers; the "verification" is deliberately
        // slow so they all pile up behind the single-flight lock.
        let mut handles = Vec::new();
        for _ in 0..12 {
            let gate = gate.clone();
            let lock = lock.clone();
            let runs = runs.clone();
            let verified = verified.clone();
            handles.push(tokio::spawn(async move {
                refresh_gate_single_flight(&gate, &lock, FINGERPRINT_TTL, || async {
                    runs.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    verified.clone()
                })
                .await
            }));
        }

        for h in handles {
            assert!(
                h.await.unwrap().writes_enabled(),
                "every caller observes the verified verdict"
            );
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "double-checked single-flight runs the verification exactly once"
        );
    }

    #[test]
    fn transfer_schema_requires_to_and_amount_and_has_confirm() {
        let schema = schema_of("lez_transfer");
        let props = schema["properties"].as_object().expect("properties");
        assert!(props.contains_key("to"));
        assert!(props.contains_key("amount"));
        assert!(props.contains_key("confirm"));
        assert!(props.contains_key("preview_token"));
        let required: Vec<&str> = schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(required.contains(&"to"));
        assert!(required.contains(&"amount"));
        // confirm must NOT be required — its absence means dry-run.
        assert!(!required.contains(&"confirm"));
    }

    #[test]
    fn faucet_schema_requires_account_id_only() {
        let schema = schema_of("lez_faucet_claim");
        let props = schema["properties"].as_object().expect("properties");
        assert!(props.contains_key("account_id"));
        assert!(props.contains_key("target_balance"));
        assert!(props.contains_key("max_claims"));
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(required, vec!["account_id"]);
    }

    #[test]
    fn read_tools_have_optional_or_single_required_params() {
        let balance = schema_of("lez_balance");
        assert!(balance["properties"].get("account_id").is_some());
        assert!(balance.get("required").is_none() || balance["required"].as_array().unwrap().is_empty());

        let fp = schema_of("lez_fingerprint");
        assert!(fp["properties"].get("sequencer_url").is_some());

        let htlc = schema_of("sepolia_htlc_status");
        let required: Vec<&str> = htlc["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(required, vec!["swap_id_or_hashlock"]);
    }

    #[test]
    fn gate_json_shapes() {
        let unverified = Gate::Unverified {
            error: "boom".into(),
        };
        assert!(!unverified.writes_enabled());
        assert_eq!(unverified.to_json()["status"], json!("unverified"));

        let embedded = crate::fingerprint::embedded_program_ids();
        let report = crate::fingerprint::build_report("http://x", &embedded, &embedded.clone());
        let verified = Gate::Verified(report.clone());
        assert!(verified.writes_enabled());
        assert_eq!(verified.to_json()["status"], json!("verified"));

        let mut remote = embedded.clone();
        remote.remove("amm");
        let bad = crate::fingerprint::build_report("http://x", &embedded, &remote);
        let mismatch = Gate::Mismatch(bad);
        assert!(!mismatch.writes_enabled());
        assert_eq!(mismatch.to_json()["report"]["matched"], json!(false));
    }

    // ── [P1-1] signed-write serialization ───────────────────────────────

    // Model the signed-write path with a slow mock "sequencer": each write
    // reads the current nonce, spends a while submitting/confirming, and only
    // then advances the nonce (exactly how the real sequencer behaves — the
    // nonce moves on COMMITMENT, not on read). Without the signed-write lock
    // held from nonce read through confirmation, concurrent writers all read
    // the same nonce and double-submit; with it, every write sees a distinct
    // nonce.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn signed_writes_serialize_from_nonce_read_through_confirmation() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let signed_write_lock = Arc::new(tokio::sync::Mutex::new(()));
        let chain_nonce = Arc::new(AtomicU64::new(0));
        let submitted_nonces = Arc::new(Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let lock = signed_write_lock.clone();
            let chain_nonce = chain_nonce.clone();
            let submitted = submitted_nonces.clone();
            handles.push(tokio::spawn(async move {
                // As in lez_transfer / the faucet auto-init: owned guard, then
                // the whole nonce-read → submit → confirm section detached.
                let guard = lock.lock_owned().await;
                run_guarded_detached(guard, async move {
                    let nonce = chain_nonce.load(Ordering::SeqCst); // nonce read
                    tokio::time::sleep(Duration::from_millis(15)).await; // slow submit+confirm
                    chain_nonce.store(nonce + 1, Ordering::SeqCst); // commitment advances it
                    submitted.lock().unwrap().push(nonce);
                })
                .await
                .expect("critical section completes");
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let mut nonces = submitted_nonces.lock().unwrap().clone();
        nonces.sort_unstable();
        assert_eq!(
            nonces,
            vec![0, 1, 2, 3],
            "every signed write must observe a distinct nonce — same-nonce \
             double submission means the lock was not held through confirmation"
        );
    }

    // ── [P2-2] request-drop survival of write critical sections ─────────

    // Dropping the MCP request future mid-submission must NOT release the
    // write lock early: the detached task keeps running, and the lock frees
    // only after the commitment ("work") resolves.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropped_request_keeps_lock_until_commitment_resolves() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let committed = Arc::new(AtomicBool::new(false));

        let guard = lock.clone().lock_owned().await;
        let committed_in_task = committed.clone();
        // The "request": awaits the detached critical section.
        let request = tokio::spawn(run_guarded_detached(guard, async move {
            // Submission is in flight; commitment resolves after a delay.
            tokio::time::sleep(Duration::from_millis(60)).await;
            committed_in_task.store(true, Ordering::SeqCst);
        }));

        // Let the detached task start, then drop the request mid-flight (MCP
        // cancellation).
        tokio::time::sleep(Duration::from_millis(10)).await;
        request.abort();

        // The critical section is detached: the lock must still be held and
        // the commitment not yet resolved.
        assert!(
            lock.clone().try_lock_owned().is_err(),
            "lock must remain held after the request is dropped"
        );
        assert!(!committed.load(Ordering::SeqCst));

        // Once the lock is grantable again, the commitment must have resolved
        // first — the next writer can never sneak in before it.
        let _next_writer = lock.lock().await;
        assert!(
            committed.load(Ordering::SeqCst),
            "lock released only after the detached critical section resolved"
        );
    }

    #[tokio::test]
    async fn run_guarded_detached_surfaces_panics_as_errors() {
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let guard = lock.clone().lock_owned().await;
        let out: Result<(), String> =
            run_guarded_detached(guard, async { panic!("boom") }).await;
        let err = out.expect_err("panic must surface as an error");
        assert!(err.contains("task failed"), "unexpected error: {err}");
        // And the lock is released — no poisoned/stuck state.
        assert!(lock.try_lock().is_ok());
    }

    #[test]
    fn result_helpers_carry_structured_and_text_content() {
        let ok = ok_json(json!({"ok": true, "x": 1}));
        assert_eq!(ok.is_error, Some(false));
        assert_eq!(ok.structured_content.as_ref().unwrap()["x"], json!(1));
        assert!(!ok.content.is_empty());

        let err = err_json(json!({"ok": false, "error": "nope"}));
        assert_eq!(err.is_error, Some(true));
        assert_eq!(
            err.structured_content.as_ref().unwrap()["error"],
            json!("nope")
        );
    }
}
