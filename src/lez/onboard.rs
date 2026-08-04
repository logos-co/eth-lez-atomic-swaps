//! Native LEZ account creation + on-chain initialization + faucet funding —
//! no `wallet` binary required.
//!
//! Three operations, layered so misuse is hard:
//!
//! 1. [`Signer::generate`] — a fresh random keypair + its derived account ID.
//!    Nothing on-chain yet; the account does not exist until initialized.
//! 2. [`ensure_initialized`] — idempotent: checks on-chain ownership first and
//!    only submits `authenticated_transfer::Instruction::Initialize` if the
//!    account isn't already owned by that program.
//! 3. [`claim_to_target`] — loops the native pinata (faucet) claim
//!    ([`crate::lez::faucet`]) until the account balance reaches a target.
//!    Calls [`ensure_initialized`] first: the sequencer SILENTLY DROPS pinata
//!    claims (and transfers) that reference a never-initialized account, so a
//!    caller cannot skip straight to claiming and get a real error — it would
//!    just spin, crediting nothing. Encoding the init-before-claim order here
//!    means callers can't get it wrong.
//!
//! All three are async and take no `&SwapConfig`/CLI assumptions — they never
//! print, and return structured results/errors so a later FFI layer can
//! surface them to a GUI onboarding flow (create account → fund it → use it),
//! exactly like [`crate::swap::progress::SwapProgress`] is forwarded today.

use std::sync::Arc;
use std::time::Duration;

use lee::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_protocol::LeeTransaction;
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde::Serialize;
use tokio::sync::{Semaphore, mpsc};

use crate::config::LezAuth;
use crate::lez::faucet;

/// Build a bare [`SequencerClient`] from a URL string.
///
/// Onboarding callers (a not-yet-configured GUI setup flow, `swap-ffi`, the
/// `onboard_maker_account` example) only have a sequencer URL and a freshly
/// generated [`Signer`] — no full `SwapConfig` (no ETH side, no LEZ HTLC
/// program ID yet, since neither exists before onboarding runs). Building a
/// dummy `SwapConfig` just to reach [`crate::lez::client::LezClient`] would
/// mean inventing unrelated fields; this mirrors `LezClient::from_raw_key`'s
/// own client construction instead, so there is exactly one way sequencer
/// clients get built in this codebase.
pub fn sequencer_client(sequencer_url: &str) -> Result<SequencerClient, String> {
    let url = url::Url::parse(sequencer_url).map_err(|e| format!("invalid sequencer URL: {e}"))?;
    SequencerClientBuilder::default()
        .build(url)
        .map_err(|e| format!("failed to create sequencer client: {e}"))
}

/// How long [`ensure_initialized`] waits for a submitted `Initialize` to
/// commit before giving up. Public-testnet blocks can be a minute or more
/// apart (see `docs/testnet.md`).
const INIT_COMMIT_TIMEOUT: Duration = Duration::from_secs(300);
const INIT_COMMIT_POLL: Duration = Duration::from_secs(3);

/// Time to wait between pinata claim attempts, letting a submitted claim land
/// before the next balance check / claim.
const CLAIM_RETRY_DELAY: Duration = Duration::from_secs(3);

/// Consecutive claim failures [`claim_to_target`] tolerates before aborting.
const MAX_CONSECUTIVE_CLAIM_FAILURES: u32 = 5;

/// Bounded retries for a single balance read in [`claim_to_target`]'s loop.
/// Public RPC endpoints occasionally drop a single request transiently (a
/// maker bot running unattended on a VPS will hit this eventually); without
/// this, one blip would abort the whole funding loop even though the very
/// next read would have succeeded. Claim submission/confirmation already has
/// its own retry semantics (the consecutive-failure counter); this covers the
/// plain read that gates the loop.
const BALANCE_READ_RETRIES: u32 = 3;
const BALANCE_READ_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Bounded retries for a single `get_account` read in [`ensure_initialized`].
/// Same rationale/values as [`BALANCE_READ_RETRIES`] (a separate constant
/// because the two reads serve different callers, not because the tuning
/// should differ) — live-verified necessary against the public testnet
/// while building the onboarding flow: `get_account` timed out transiently
/// on 2 of 3 real runs against `testnet.lez.logos.co`, both on the very
/// first (previously unretried) read.
const ACCOUNT_READ_RETRIES: u32 = 3;
const ACCOUNT_READ_RETRY_DELAY: Duration = Duration::from_secs(2);

/// How many candidate keys [`Signer::generate`] will try before giving up.
/// `PrivateKey::try_new` rejecting 32 cryptographically random bytes is
/// astronomically unlikely; the cap only turns a pathological RNG failure
/// into an error instead of an infinite loop.
const MAX_KEYGEN_ATTEMPTS: u32 = 8;

// ── Signer ──────────────────────────────────────────────────────────────

/// A LEZ signing identity: a keypair plus its derived account ID.
///
/// This is the one canonical `Signer` type for the app: `lez-mcp` re-exports
/// it (see `lez-mcp/src/server.rs`) rather than defining its own.
pub struct Signer {
    pub account_id: AccountId,
    pub signing_key: PrivateKey,
}

impl Signer {
    fn from_key_bytes(bytes: [u8; 32]) -> Result<Self, String> {
        let signing_key =
            PrivateKey::try_new(bytes).map_err(|e| format!("invalid LEZ private key: {e}"))?;
        let public_key = PublicKey::new_from_private_key(&signing_key);
        Ok(Self {
            account_id: AccountId::from(&public_key),
            signing_key,
        })
    }

    /// Create a brand-new account: a fresh random signing key and its derived
    /// account ID. Nothing is submitted on-chain — the account does not exist
    /// until [`ensure_initialized`] runs (and typically has zero balance until
    /// [`claim_to_target`] funds it). This is the entry point for in-app
    /// account creation (no external `wallet` binary, no scaffold).
    pub fn generate() -> Result<Self, String> {
        let mut last_err = String::new();
        for _ in 0..MAX_KEYGEN_ATTEMPTS {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes)
                .map_err(|e| format!("failed to read system randomness: {e}"))?;
            match Self::from_key_bytes(bytes) {
                Ok(signer) => return Ok(signer),
                Err(e) => last_err = e,
            }
        }
        Err(format!(
            "failed to generate a valid LEZ signing key after {MAX_KEYGEN_ATTEMPTS} attempts: \
             {last_err}"
        ))
    }

    /// Construct from a raw 32-byte hex-encoded signing key.
    pub fn from_raw_key(hex_key: &str) -> Result<Self, String> {
        let bytes: [u8; 32] = hex::decode(hex_key.trim())
            .map_err(|e| format!("invalid LEZ signing key hex: {e}"))?
            .try_into()
            .map_err(|_| "LEZ signing key must be 32 bytes".to_string())?;
        Self::from_key_bytes(bytes)
    }

    /// Resolve a signer from the app's `LezAuth` config: a raw hex key, or a
    /// scaffold-managed wallet account.
    pub fn from_auth(auth: &LezAuth) -> Result<Self, String> {
        match auth {
            LezAuth::RawKey(hex_key) => Self::from_raw_key(hex_key),
            LezAuth::Wallet { home, account_id } => {
                let wc = crate::scaffold::wallet_core(home).map_err(|e| format!("wallet home: {e}"))?;
                let signing_key = wc
                    .get_account_public_signing_key(*account_id)
                    .ok_or_else(|| {
                        format!(
                            "wallet has no signing key for account {}",
                            crate::config::account_id_to_base58(account_id)
                        )
                    })?
                    .clone();
                Ok(Self {
                    account_id: *account_id,
                    signing_key,
                })
            }
        }
    }
}

// ── Initialization ──────────────────────────────────────────────────────

/// Outcome of [`ensure_initialized`].
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", content = "data")]
pub enum InitOutcome {
    /// The account was already owned by the auth-transfer program — no
    /// transaction was submitted.
    AlreadyInitialized,
    /// `Initialize` was submitted and confirmed committed.
    Initialized { tx_hash: String },
}

/// Idempotently ensure `signer`'s account is initialized (owned by the
/// `authenticated_transfer` program) on-chain.
///
/// CRITICAL PROTOCOL FACT: the sequencer silently drops transactions that
/// reference a never-initialized account — both pinata claims and transfers.
/// Every native flow that will claim/transfer to a fresh account MUST call
/// this first; that guard is exactly the check this function performs.
///
/// Safe to call repeatedly / concurrently with itself for the *same* signer
/// from a single caller — it re-reads ownership before submitting, so a
/// second call after a first has already initialized returns
/// [`InitOutcome::AlreadyInitialized`] without submitting anything. It is
/// NOT safe to race two independent callers signing for the same account at
/// once (they could both observe "not yet initialized" and both submit); the
/// standing MCP server serializes this itself (`Ctx::signed_write_lock`) for
/// that reason.
pub async fn ensure_initialized(
    sequencer: &SequencerClient,
    signer: &Signer,
) -> Result<InitOutcome, String> {
    let program_id = programs::authenticated_transfer().id();

    let existing = {
        let mut last_err = String::new();
        let mut result = None;
        for attempt in 0..ACCOUNT_READ_RETRIES {
            match sequencer.get_account(signer.account_id).await {
                Ok(account) => {
                    result = Some(account);
                    break;
                }
                Err(e) => {
                    last_err = format!("get_account failed: {e}");
                    if attempt + 1 < ACCOUNT_READ_RETRIES {
                        tokio::time::sleep(ACCOUNT_READ_RETRY_DELAY).await;
                    }
                }
            }
        }
        result.ok_or_else(|| format!("{last_err} (after {ACCOUNT_READ_RETRIES} attempts)"))?
    };
    if existing.program_owner == program_id {
        return Ok(InitOutcome::AlreadyInitialized);
    }

    let nonces = sequencer
        .get_accounts_nonces(vec![signer.account_id])
        .await
        .map_err(|e| format!("get_accounts_nonces failed: {e}"))?;

    let message = Message::try_new(
        program_id,
        vec![signer.account_id],
        nonces,
        authenticated_transfer_core::Instruction::Initialize,
    )
    .map_err(|e| format!("failed to build Initialize message: {e}"))?;
    let witness_set = WitnessSet::for_message(&message, &[&signer.signing_key]);
    let tx = PublicTransaction::new(message, witness_set);

    let tx_hash = sequencer
        .send_transaction(LeeTransaction::Public(tx))
        .await
        .map_err(|e| format!("Initialize submission failed: {e}"))?
        .to_string();

    // Poll until ownership actually flips, or time out. Ownership (not merely
    // "the tx was accepted") is the effect we need: it is the same guard
    // `ensure_initialized`'s own idempotency check reads.
    let deadline = tokio::time::Instant::now() + INIT_COMMIT_TIMEOUT;
    loop {
        // A transient read error here is treated the same as "not yet
        // committed" rather than aborting the whole confirmation wait: the
        // deadline above already bounds how long this can run, and a single
        // dropped request (measured against the public testnet) must not
        // fail an Initialize that actually landed.
        match sequencer.get_account(signer.account_id).await {
            Ok(account) if account.program_owner == program_id => {
                return Ok(InitOutcome::Initialized { tx_hash });
            }
            Ok(_) | Err(_) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "account Initialize did not commit within {}s (tx {tx_hash})",
                INIT_COMMIT_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(INIT_COMMIT_POLL).await;
    }
}

// ── Faucet funding ──────────────────────────────────────────────────────

/// Progress events emitted by [`claim_to_target`]. Serializable so a caller
/// can forward these straight to a UI, mirroring `SwapProgress` — including
/// its `#[serde(tag = "step", content = "data")]` wire shape (not just the
/// derive), so `swap-ffi`/`swap-module`'s existing progress-envelope code
/// (which reads a `step` key) forwards `lez_setup.progress` events exactly
/// like `maker.progress`/`taker.progress` with no separate parsing path.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "step", content = "data")]
pub enum FundingProgress {
    /// Ensuring the account is initialized before any claim is attempted.
    Initializing,
    Initialized { tx_hash: String },
    AlreadyInitialized,
    /// Balance checked; still below target — about to attempt a claim.
    CheckingBalance { balance: u128, target: u128 },
    /// Target reached; the loop is returning.
    TargetReached { balance: u128 },
    /// A claim attempt succeeded and committed.
    Claimed { tx_hash: String, total_claims: u64 },
    /// A claim attempt failed; will retry unless the failure cap is hit.
    ClaimFailed {
        error: String,
        consecutive_failures: u32,
    },
}

pub type FundingProgressSender = mpsc::UnboundedSender<FundingProgress>;

fn emit(sender: Option<&FundingProgressSender>, event: FundingProgress) {
    if let Some(sender) = sender {
        // The receiver may have been dropped (caller uninterested); that is
        // not an error for the funding loop.
        let _ = sender.send(event);
    }
}

/// Claim from the native pinata faucet (150 LEZ/claim) until `signer`'s
/// balance reaches `target`, or abort after
/// [`MAX_CONSECUTIVE_CLAIM_FAILURES`] consecutive claim failures. Returns the
/// final observed balance.
///
/// Calls [`ensure_initialized`] first (see its docs for why: the sequencer
/// silently drops claims against a never-initialized account) — a caller
/// cannot get the ordering wrong by construction.
///
/// Cancellation: this is a plain `async fn` with no detached work — dropping
/// the returned future stops the loop at its next await point (a claim
/// already submitted still lands on-chain, same as any other LEZ tx; this
/// mirrors how `bot::fund_to_target` behaved before this lift).
pub async fn claim_to_target(
    sequencer: &SequencerClient,
    signer: &Signer,
    target: u128,
    progress: Option<FundingProgressSender>,
) -> Result<u128, String> {
    emit(progress.as_ref(), FundingProgress::Initializing);
    match ensure_initialized(sequencer, signer).await? {
        InitOutcome::AlreadyInitialized => {
            emit(progress.as_ref(), FundingProgress::AlreadyInitialized)
        }
        InitOutcome::Initialized { tx_hash } => {
            emit(progress.as_ref(), FundingProgress::Initialized { tx_hash })
        }
    }

    // A single-permit local semaphore: claim_to_target only ever runs one
    // solve at a time for this signer, but solve_claim's API takes an
    // Arc<Semaphore> (shared PoW concurrency guard) so it can be reused
    // unchanged from lez-mcp's caller, which shares one across many signers.
    let pow_permits = Arc::new(Semaphore::new(1));

    let mut consecutive_failures: u32 = 0;
    let mut claims: u64 = 0;
    loop {
        let balance = read_balance_retrying(sequencer, signer.account_id).await?;
        if balance >= target {
            emit(progress.as_ref(), FundingProgress::TargetReached { balance });
            return Ok(balance);
        }
        emit(
            progress.as_ref(),
            FundingProgress::CheckingBalance { balance, target },
        );

        match attempt_claim(sequencer, signer.account_id, &pow_permits).await {
            Ok(tx_hash) => {
                consecutive_failures = 0;
                claims += 1;
                emit(
                    progress.as_ref(),
                    FundingProgress::Claimed {
                        tx_hash,
                        total_claims: claims,
                    },
                );
            }
            Err(error) => {
                consecutive_failures += 1;
                emit(
                    progress.as_ref(),
                    FundingProgress::ClaimFailed {
                        error: error.clone(),
                        consecutive_failures,
                    },
                );
                if consecutive_failures >= MAX_CONSECUTIVE_CLAIM_FAILURES {
                    return Err(format!(
                        "{MAX_CONSECUTIVE_CLAIM_FAILURES} consecutive pinata claim failures — \
                         aborting funding loop (last error: {error})"
                    ));
                }
            }
        }
        // Let the claim land before re-checking the balance.
        tokio::time::sleep(CLAIM_RETRY_DELAY).await;
    }
}

/// Read an account balance, retrying up to [`BALANCE_READ_RETRIES`] times on
/// transient RPC errors before giving up.
async fn read_balance_retrying(
    sequencer: &SequencerClient,
    account_id: AccountId,
) -> Result<u128, String> {
    let mut last_err = String::new();
    for attempt in 0..BALANCE_READ_RETRIES {
        match sequencer.get_account_balance(account_id).await {
            Ok(balance) => return Ok(balance),
            Err(e) => {
                last_err = format!("get_account_balance failed: {e}");
                if attempt + 1 < BALANCE_READ_RETRIES {
                    tokio::time::sleep(BALANCE_READ_RETRY_DELAY).await;
                }
            }
        }
    }
    Err(format!(
        "{last_err} (after {BALANCE_READ_RETRIES} attempts)"
    ))
}

async fn attempt_claim(
    sequencer: &SequencerClient,
    winner: AccountId,
    pow_permits: &Arc<Semaphore>,
) -> Result<String, String> {
    let solved = faucet::solve_claim(sequencer, winner, pow_permits.clone()).await?;
    let submission = faucet::submit_solved_claim(sequencer, &solved).await?;
    Ok(submission.tx_hash)
}
