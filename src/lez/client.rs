use lee::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use lee_core::{
    account::Nonce,
    program::{PdaSeed, ProgramId},
};
use lez_htlc_program::{HTLCEscrow, HTLCInstruction, HTLCState};
use sequencer_service_protocol::LeeTransaction;
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use tracing::{debug, info, warn};
use url::Url;

use crate::{
    config::{LezAuth, SwapConfig},
    error::{Result, SwapError},
    scaffold,
};

/// Terminal outcome of a confirmed LEZ refund attempt.
///
/// See [`LezClient::refund_confirmed`].
#[derive(Debug)]
pub enum RefundOutcome {
    /// Escrow observed in the `Refunded` state. Carries the submit tx hash when
    /// this call issued the refund (empty if it was already refunded).
    Refunded(String),
    /// A taker claim won the race: the escrow is `Claimed` and carries the
    /// revealed 32-byte preimage. The maker must claim the ETH side.
    ClaimedByTaker([u8; 32]),
}

/// Bounded retries / initial backoff for [`LezClient::get_balance_with_retry`].
/// Shared by every hot-path balance read that can tolerate a few extra seconds
/// of latency in exchange for not treating a transient sequencer blip as a
/// hard failure: the maker-loop startup guard, the per-iteration balance
/// check, and the background fund-topper.
const BALANCE_RETRY_ATTEMPTS: u32 = 5;
const BALANCE_RETRY_INITIAL_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// Generic core of [`LezClient::get_balance_with_retry`], parameterized over
/// the fetch so the retry / backoff / `on_transient` behaviour is
/// unit-testable without a live sequencer (same pattern as
/// `cli::bot::read_escrow_bounded_with` / `max_escrow_balance_with`).
///
/// `on_transient` fires exactly once per call IF at least one attempt failed
/// — whether the read went on to succeed on a later attempt or exhausted
/// every attempt — never more than once, and never when the very first
/// attempt succeeds cleanly.
async fn balance_with_retry_core<F, Fut>(
    mut fetch: F,
    mut on_transient: impl FnMut(&SwapError),
) -> Result<u128>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<u128>>,
{
    let mut delay = BALANCE_RETRY_INITIAL_DELAY;
    let mut last_err = None;
    let mut retried = false;
    for attempt in 1..=BALANCE_RETRY_ATTEMPTS {
        match fetch().await {
            Ok(balance) => {
                if retried {
                    // We only reach here with `last_err` populated, so this
                    // unwrap is safe; kept as an expect for clarity.
                    on_transient(
                        last_err
                            .as_ref()
                            .expect("retried implies at least one Err was recorded"),
                    );
                }
                return Ok(balance);
            }
            Err(e) => {
                retried = true;
                warn!(
                    "balance read failed (attempt {attempt}/{BALANCE_RETRY_ATTEMPTS}): {e} — \
                     retrying (this means the sequencer could not be reached, NOT that the \
                     account is under-funded)"
                );
                last_err = Some(e);
                if attempt < BALANCE_RETRY_ATTEMPTS {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(std::time::Duration::from_secs(30));
                }
            }
        }
    }
    let err = last_err.expect("loop runs at least once, so an all-Err path always sets this");
    on_transient(&err);
    Err(err)
}

enum LezBackend {
    Standalone {
        sequencer: SequencerClient,
        private_key: PrivateKey,
    },
    Wallet {
        wallet_core: Box<wallet::WalletCore>,
        private_key: PrivateKey,
        /// When set (LEZ_SEQUENCER_URL was explicit), overrides the wallet
        /// config's sequencer so a hosted/public sequencer can be targeted.
        sequencer_override: Option<SequencerClient>,
    },
}

pub struct LezClient {
    backend: LezBackend,
    account_id: AccountId,
    program_id: ProgramId,
    poll_interval: std::time::Duration,
}

/// Classification of one escrow read during refund confirmation.
///
/// Extracted (and unit-tested) to pin the P1-C invariant: an absent (`None`)
/// read is `Absent`, never `Refunded`. The LEZ HTLC program's refund leaves the
/// account with `state == Refunded` and intact data — it never deletes it — so
/// `get_escrow` returning `None` means a missing/short/phantom read, which is
/// NOT a terminal refund and must be retried, not acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefundReadClass {
    Refunded,
    ClaimedByTaker,
    StillLocked,
    Absent,
}

fn classify_refund_read(state: Option<HTLCState>) -> RefundReadClass {
    match state {
        None => RefundReadClass::Absent,
        Some(HTLCState::Refunded) => RefundReadClass::Refunded,
        Some(HTLCState::Claimed) => RefundReadClass::ClaimedByTaker,
        Some(HTLCState::Locked) => RefundReadClass::StillLocked,
    }
}

/// Whether a fresh [`LezClient::lock`] must REFUSE because an escrow already
/// exists for this hashlock. Any existing escrow — in ANY state — means the
/// hashlock is already in flight (a replayed/retained swap, or a partially
/// completed prior lock). Funding a fresh transfer would strand the excess, so
/// there is exactly one legitimate funder per hashlock and a pre-existing PDA
/// means we are not it (P1-A suspenders / check-before-fund).
fn existing_escrow_blocks_fresh_lock(existing: Option<HTLCState>) -> bool {
    existing.is_some()
}

/// Whether an escrow observed during [`LezClient::lock`]'s confirmation poll is
/// conclusively OUR just-submitted lock (principle i): state `Locked` and
/// maker/taker/amount/timelock exactly as submitted. Any other escrow at this
/// PDA is pre-existing (the single-read check-before-fund can be defeated by one
/// phantom `None`) and funding it would strand the transfer.
///
/// The `maker_id == self` check (P1-3) is load-bearing: a different maker's
/// escrow sharing our hashlock/taker/amount/timelock would otherwise pass, and
/// Step 2 would fund THEIR escrow — one only they can refund. Confirmation
/// requires that WE are the recorded maker.
fn escrow_confirms_our_lock(
    escrow: &HTLCEscrow,
    maker_id: &AccountId,
    taker_id: &AccountId,
    amount: u128,
    timelock_ms: u64,
) -> bool {
    escrow.state == HTLCState::Locked
        && escrow.maker_id == *maker_id
        && escrow.taker_id == *taker_id
        && escrow.amount == amount
        && escrow.timelock == timelock_ms
}

/// Build the LEZ `Lock` instruction. `taker_id` becomes `escrow.taker_id`, the
/// ONLY account whose signature `execute_claim` accepts — so for an ETH→LEZ
/// swap it must be the account the taker published in its own ETH lock, never a
/// statically configured one.
///
/// Extracted (and public) so an integration test can assert the whole chain —
/// on-chain `Locked.takerLezAccount` → watcher decode → maker classification →
/// this instruction's `taker_id` — without a live sequencer. The LEZ timelock is
/// milliseconds on the wire; seconds everywhere else in this app.
pub fn build_lock_instruction(
    hashlock: [u8; 32],
    taker_id: AccountId,
    amount: u128,
    timelock_secs: u64,
) -> HTLCInstruction {
    HTLCInstruction::Lock {
        hashlock,
        taker_id,
        amount,
        timelock: timelock_secs * 1000,
    }
}

impl LezClient {
    /// Create a LezClient from a SwapConfig. Dispatches based on `LezAuth` variant:
    /// - `RawKey`: uses the hex-encoded signing key directly (tests / legacy).
    /// - `Wallet`: reads the signing key from a scaffold-managed wallet on disk.
    pub fn new(config: &SwapConfig) -> Result<Self> {
        match &config.lez_auth {
            LezAuth::RawKey(hex_key) => Self::from_raw_key(hex_key, config),
            LezAuth::Wallet { home, account_id } => Self::from_wallet(home, account_id, config),
        }
    }

    /// Construct from a raw hex-encoded signing key (32 bytes). Used by tests and
    /// the in-process demo environment.
    pub fn from_raw_key(hex_key: &str, config: &SwapConfig) -> Result<Self> {
        let key_bytes: [u8; 32] = hex::decode(hex_key)
            .map_err(|e| SwapError::InvalidConfig(format!("invalid LEZ signing key hex: {e}")))?
            .try_into()
            .map_err(|_| SwapError::InvalidConfig("LEZ signing key must be 32 bytes".into()))?;

        let private_key = PrivateKey::try_new(key_bytes)
            .map_err(|e| SwapError::InvalidConfig(format!("invalid LEZ private key: {e}")))?;

        let public_key = PublicKey::new_from_private_key(&private_key);
        let account_id = AccountId::from(&public_key);

        let sequencer_url = Url::parse(&config.lez_sequencer_url)
            .map_err(|e| SwapError::InvalidConfig(format!("invalid sequencer URL: {e}")))?;

        let sequencer = SequencerClientBuilder::default()
            .build(sequencer_url)
            .map_err(|e| SwapError::LezSequencer(format!("failed to create client: {e}")))?;

        Ok(Self {
            backend: LezBackend::Standalone {
                sequencer,
                private_key,
            },
            account_id,
            program_id: config.lez_htlc_program_id,
            poll_interval: config.poll_interval,
        })
    }

    /// Construct from a scaffold-managed wallet. Reads the signing key for the
    /// given account from the wallet config on disk. Uses the WalletCore's
    /// sequencer client by default, unless `LEZ_SEQUENCER_URL` was explicitly
    /// set — in which case that URL overrides the wallet config's sequencer.
    pub fn from_wallet(
        wallet_home: &std::path::Path,
        target_account_id: &AccountId,
        config: &SwapConfig,
    ) -> Result<Self> {
        let wc = scaffold::wallet_core(wallet_home)?;

        let private_key = wc
            .get_account_public_signing_key(*target_account_id)
            .ok_or_else(|| {
                SwapError::Scaffold(format!(
                    "wallet has no signing key for account {}",
                    target_account_id
                ))
            })?
            .clone();

        // An explicitly-set LEZ_SEQUENCER_URL takes precedence over the wallet
        // config's sequencer_addr, so users can retarget a hosted/public
        // sequencer via env. Falls back to the wallet's own client otherwise.
        let sequencer_override = if config.lez_sequencer_url_explicit {
            let url = Url::parse(&config.lez_sequencer_url)
                .map_err(|e| SwapError::InvalidConfig(format!("invalid sequencer URL: {e}")))?;
            let client = SequencerClientBuilder::default()
                .build(url)
                .map_err(|e| SwapError::LezSequencer(format!("failed to create client: {e}")))?;
            Some(client)
        } else {
            None
        };

        Ok(Self {
            backend: LezBackend::Wallet {
                wallet_core: Box::new(wc),
                private_key,
                sequencer_override,
            },
            account_id: *target_account_id,
            program_id: config.lez_htlc_program_id,
            poll_interval: config.poll_interval,
        })
    }

    fn sequencer(&self) -> &SequencerClient {
        match &self.backend {
            LezBackend::Standalone { sequencer, .. } => sequencer,
            LezBackend::Wallet {
                wallet_core,
                sequencer_override,
                ..
            } => sequencer_override
                .as_ref()
                .unwrap_or(&wallet_core.sequencer_client),
        }
    }

    fn private_key(&self) -> &PrivateKey {
        match &self.backend {
            LezBackend::Standalone { private_key, .. } => private_key,
            LezBackend::Wallet { private_key, .. } => private_key,
        }
    }

    /// Derive the escrow PDA account ID from a hashlock.
    pub fn escrow_pda(&self, hashlock: &[u8; 32]) -> AccountId {
        AccountId::for_public_pda(&self.program_id, &PdaSeed::new(*hashlock))
    }

    /// Read the escrow PDA state. Returns `None` if the account doesn't exist
    /// or contains invalid/phantom data.
    pub async fn get_escrow(&self, hashlock: &[u8; 32]) -> Result<Option<HTLCEscrow>> {
        let pda = self.escrow_pda(hashlock);
        let resp = self
            .sequencer()
            .get_account(pda)
            .await
            .map_err(|e| SwapError::LezSequencer(format!("get_account failed: {e}")))?;

        let data: Vec<u8> = resp.data.into();
        eprintln!(
            "[get_escrow] pda={} data_len={}",
            hex::encode(pda.value()),
            data.len()
        );
        if data.len() < 125 {
            eprintln!("[get_escrow] data too short ({} < 125)", data.len());
            return Ok(None);
        }

        let escrow = HTLCEscrow::from_bytes(&data);

        // The sequencer returns data for non-existent PDAs. Verify the stored
        // hashlock matches what we queried for to reject phantom accounts.
        if escrow.hashlock != *hashlock {
            eprintln!(
                "[get_escrow] hashlock mismatch: expected={} got={}",
                hex::encode(hashlock),
                hex::encode(escrow.hashlock),
            );
            return Ok(None);
        }

        Ok(Some(escrow))
    }

    /// Read the balance of an account.
    pub async fn get_balance(&self, account_id: &AccountId) -> Result<u128> {
        let resp = self
            .sequencer()
            .get_account_balance(*account_id)
            .await
            .map_err(|e| SwapError::LezSequencer(format!("get_account_balance failed: {e}")))?;

        Ok(resp)
    }

    /// [`Self::get_balance`], bounded-retried with exponential backoff on the
    /// `Err` path only (issue #93 point 2, generalized for the follow-up gap
    /// found live on the deployed maker: the startup inventory read got this
    /// treatment, but every OTHER hot-path balance read — the maker loop's
    /// per-iteration check and the fund-topper's read — still did a single
    /// unretried call, so a transient sequencer timeout burned a whole loop
    /// iteration and inflated the failure counter).
    ///
    /// A transient sequencer error (a timeout, a dropped connection) is NOT the
    /// same thing as "reachable and genuinely under-funded" — the latter is a
    /// successful read that just reports a low number, and this function does
    /// NOT retry that case; the caller still sees it immediately via `Ok`.
    /// Only the `Err` case is retried, mirroring the retry `claim_to_target`
    /// got in #77.
    ///
    /// `on_transient` is invoked exactly once per call IF at least one attempt
    /// failed — whether the read went on to succeed on a later attempt or
    /// exhausted every attempt — so callers can track "the sequencer blipped"
    /// as a signal distinct from the read's ultimate `Result`, e.g. to bump an
    /// operator-facing `transient_errors` counter without polluting a
    /// swap-failure counter (a retried-and-recovered blip is not a swap
    /// failure, and neither is a fully-exhausted one — it is the sequencer
    /// being unreachable, not the swap logic doing anything wrong).
    ///
    /// Live-observed on the VPS: repeated `get_account_balance` timeouts,
    /// sometimes recovering within the retry budget and sometimes not. Without
    /// this retry, a single-shot read turned a sub-second sequencer blip into
    /// either a crash loop (the startup case, #93) or a fully-counted "swap
    /// failure" (the per-iteration case) with no offers on the board in
    /// between.
    pub async fn get_balance_with_retry(
        &self,
        account_id: &AccountId,
        on_transient: impl FnMut(&SwapError),
    ) -> Result<u128> {
        balance_with_retry_core(|| self.get_balance(account_id), on_transient).await
    }

    /// Transfer LEZ to a recipient using the authenticated transfer program.
    pub async fn transfer(&self, recipient: AccountId, amount: u128) -> Result<String> {
        let program_id = programs::authenticated_transfer().id();
        let account_ids = vec![self.account_id, recipient];

        let nonces = self.get_nonces(&[self.account_id]).await?;

        // v0.2.0: the transfer instruction is a typed enum, not a bare u128.
        let instruction = authenticated_transfer_core::Instruction::Transfer { amount };
        let message = Message::try_new(program_id, account_ids, nonces, instruction)
            .map_err(|e| SwapError::LezTransaction(format!("failed to build message: {e}")))?;

        let witness_set = WitnessSet::for_message(&message, &[self.private_key()]);
        let tx = PublicTransaction::new(message, witness_set);

        let tx_hash = self
            .sequencer()
            .send_transaction(LeeTransaction::Public(tx))
            .await
            .map_err(|e| SwapError::LezTransaction(format!("transfer failed: {e}")))?;

        let tx_hash = tx_hash.to_string();
        info!(tx_hash = %tx_hash, amount, "LEZ transfer submitted");
        Ok(tx_hash)
    }

    /// Lock LEZ into the HTLC escrow PDA.
    ///
    /// Two-step: first submits the Lock instruction (which claims the PDA and
    /// stores escrow metadata), then transfers funds to the PDA.
    pub async fn lock(
        &self,
        hashlock: [u8; 32],
        taker_id: AccountId,
        amount: u128,
        timelock_secs: u64,
    ) -> Result<String> {
        let pda = self.escrow_pda(&hashlock);

        // Check-before-fund (P1-A suspenders): refuse to fund an escrow whose PDA
        // already exists. A pre-existing escrow for this hashlock means it is
        // already being processed (a replayed/retained in-flight swap, or a
        // partially-completed prior lock) — the confirmation poll below would see
        // the existing PDA immediately and Step 2 would transfer `amount` into it
        // a SECOND time, stranding the excess. There is exactly one legitimate
        // funder per hashlock; if the PDA is already there, we are not it.
        let existing = self.get_escrow(&hashlock).await?;
        if existing_escrow_blocks_fresh_lock(existing.as_ref().map(|e| e.state)) {
            let balance = self.get_balance(&pda).await.unwrap_or(0);
            return Err(SwapError::InvalidState {
                expected: "uninitialized escrow PDA before lock".into(),
                actual: format!(
                    "escrow PDA {} already exists (state {:?}, balance {balance}) — refusing to \
                     re-fund; this hashlock is already in flight",
                    hex::encode(pda.value()),
                    existing.map(|e| e.state),
                ),
            });
        }

        // Step 1: Lock — claims the uninitialized PDA and stores escrow data.
        let instruction = build_lock_instruction(hashlock, taker_id, amount, timelock_secs);

        let lock_hash = self
            .send_htlc_instruction(vec![self.account_id, pda], instruction)
            .await?;
        debug!(tx_hash = %lock_hash, "LEZ HTLC lock submitted");

        // Wait for the lock to be committed before funding. Public-testnet
        // blocks can be a minute or more apart, so allow several blocks.
        //
        // Principle (i) hardening: "an escrow appeared" is NOT conclusive
        // confirmation of OUR lock. If the single-read check-before-fund above
        // hit a phantom `None` for a PRE-EXISTING escrow, treating any existing
        // PDA here as our own confirmation would transfer `amount` into an
        // escrow we do not control (or a terminal one) and strand it. Only an
        // escrow whose state/taker/amount/timelock match EXACTLY what we just
        // submitted confirms our lock; any mismatch refuses to fund.
        let expected_timelock_ms = timelock_secs * 1000;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if let Some(escrow) = self.get_escrow(&hashlock).await? {
                if !escrow_confirms_our_lock(
                    &escrow,
                    &self.account_id,
                    &taker_id,
                    amount,
                    expected_timelock_ms,
                ) {
                    return Err(SwapError::InvalidState {
                        expected: format!(
                            "our just-submitted Locked escrow (amount {amount}, timelock \
                             {expected_timelock_ms}ms)"
                        ),
                        actual: format!(
                            "pre-existing/mismatched escrow at PDA {} (state {:?}, amount {}, \
                             timelock {}ms) — refusing to fund",
                            hex::encode(pda.value()),
                            escrow.state,
                            escrow.amount,
                            escrow.timelock,
                        ),
                    });
                }
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(SwapError::Timeout("LEZ lock confirmation".into()));
            }
            tokio::time::sleep(self.poll_interval).await;
        }

        // Step 2: Fund the escrow PDA (now owned by the HTLC program).
        let transfer_hash = self.transfer(pda, amount).await?;
        debug!(tx_hash = %transfer_hash, "escrow PDA funding submitted");

        // Confirm the funding transfer actually landed. The sequencer accepts
        // the transfer eagerly but can still reject it during execution (e.g.
        // "Guest panicked: Sender has insufficient balance"). Without this
        // check a rejected transfer looks like success and both peers wait
        // forever on a zero-balance escrow. Poll the PDA balance until it
        // reaches the locked amount, or fail loudly. Generous deadline for
        // public-testnet block cadence (~1 min/block).
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            let balance = self.get_balance(&pda).await.unwrap_or(0);
            if balance >= amount {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(SwapError::LezTransaction(format!(
                    "LEZ escrow funding transfer did not land (PDA balance {balance} < {amount}) \
                     — check maker balance (need >= {amount}) and .scaffold/logs/sequencer.log"
                )));
            }
            tokio::time::sleep(self.poll_interval).await;
        }

        info!(lock_tx = %lock_hash, fund_tx = %transfer_hash, "LEZ HTLC locked and funded");
        Ok(lock_hash)
    }

    /// Claim LEZ from the HTLC escrow by revealing the preimage.
    pub async fn claim(&self, hashlock: &[u8; 32], preimage: &[u8; 32]) -> Result<String> {
        let pda = self.escrow_pda(hashlock);

        let instruction = HTLCInstruction::Claim {
            preimage: preimage.to_vec(),
        };

        let tx_hash = self
            .send_htlc_instruction(vec![self.account_id, pda], instruction)
            .await?;

        info!(tx_hash = %tx_hash, "LEZ HTLC claimed");
        Ok(tx_hash)
    }

    /// Refund LEZ from the HTLC escrow back to the maker.
    pub async fn refund(&self, hashlock: &[u8; 32]) -> Result<String> {
        let pda = self.escrow_pda(hashlock);

        let tx_hash = self
            .send_htlc_instruction(vec![self.account_id, pda], HTLCInstruction::Refund)
            .await?;

        info!(tx_hash = %tx_hash, "LEZ HTLC refunded");
        Ok(tx_hash)
    }

    /// Refund LEZ and wait until the escrow reaches a terminal on-chain state.
    ///
    /// The bare [`refund`](Self::refund) returns as soon as the transaction is
    /// *submitted* — it is not yet committed and can still lose a race to a
    /// last-moment taker claim, or be rejected during execution. Callers that
    /// journal in-flight swaps must not drop the entry until the escrow is
    /// confirmed terminal, otherwise a rejected refund strands locked LEZ (and,
    /// if the taker actually claimed, the maker still owes itself an ETH claim
    /// with the revealed preimage). This mirrors the confirmation polling in
    /// [`lock`](Self::lock).
    ///
    /// Returns:
    /// - [`RefundOutcome::Refunded`] once the escrow is observed `Refunded`.
    /// - [`RefundOutcome::ClaimedByTaker`] if a taker claim won the race (the
    ///   escrow is `Claimed` and carries the revealed preimage) — the caller
    ///   should claim the ETH side instead of treating this as a plain refund.
    /// - `Err` if neither terminal state is reached before the deadline (the
    ///   caller must keep the journal entry and retry on the next restart).
    pub async fn refund_confirmed(&self, hashlock: &[u8; 32]) -> Result<RefundOutcome> {
        // Max consecutive absent/phantom reads to tolerate before surfacing
        // Unknown. `get_escrow` returns `None` for missing/short/mismatched data
        // — which, critically, is NOT a refund: the LEZ HTLC program leaves the
        // account in place with `state == Refunded` on refund (it does not delete
        // it, see programs/lez-htlc execute_refund). So a `None` read is a
        // phantom/stale sequencer response and must NEVER be treated as terminal
        // — doing so during a refund-vs-claim race would drop the journal entry
        // and lose the preimage/ETH path (P1-C).
        const MAX_ABSENT_READS: u32 = 5;

        let mut submitted = false;
        let mut submit_tx: Option<String> = None;
        let mut submit_err: Option<String> = None;
        let mut absent_reads: u32 = 0;

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            let escrow = self.get_escrow(hashlock).await?;
            match classify_refund_read(escrow.as_ref().map(|e| e.state)) {
                RefundReadClass::Refunded => {
                    return Ok(RefundOutcome::Refunded(submit_tx.unwrap_or_default()));
                }
                RefundReadClass::ClaimedByTaker => {
                    // Safe: classify only returns this for Some(Claimed).
                    return Self::claimed_outcome(escrow.as_ref().expect("claimed escrow present"));
                }
                RefundReadClass::StillLocked => {
                    absent_reads = 0;
                    // Submit the refund exactly once; a concurrent taker claim can
                    // still make it revert, so we keep polling for the terminal
                    // state regardless of the submit result.
                    if !submitted {
                        let submit = self.refund(hashlock).await;
                        submit_tx = submit.as_ref().ok().cloned();
                        submit_err = submit.as_ref().err().map(|e| e.to_string());
                        submitted = true;
                    }
                }
                RefundReadClass::Absent => {
                    absent_reads += 1;
                    debug!(
                        hashlock = %hex::encode(hashlock),
                        "refund_confirmed: escrow read absent ({absent_reads}/{MAX_ABSENT_READS}) \
                         — NOT terminal (refunds keep the account); retrying"
                    );
                    if absent_reads >= MAX_ABSENT_READS {
                        return Err(SwapError::EscrowStateUnknown(format!(
                            "escrow {} read absent {absent_reads}x (never a confirmed refund) — \
                             retaining journal for retry",
                            hex::encode(hashlock),
                        )));
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(SwapError::EscrowStateUnknown(match &submit_err {
                    Some(m) => {
                        format!("LEZ refund not confirmed before deadline (submit failed: {m})")
                    }
                    None => {
                        "LEZ refund not confirmed before deadline (escrow state unknown)".into()
                    }
                }));
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Extract the revealed preimage from a `Claimed` escrow.
    fn claimed_outcome(escrow: &HTLCEscrow) -> Result<RefundOutcome> {
        let preimage = escrow
            .preimage
            .clone()
            .and_then(|p| <[u8; 32]>::try_from(p).ok())
            .ok_or_else(|| SwapError::InvalidState {
                expected: "revealed 32-byte preimage on claimed escrow".into(),
                actual: "missing or malformed preimage".into(),
            })?;
        Ok(RefundOutcome::ClaimedByTaker(preimage))
    }

    pub fn account_id(&self) -> AccountId {
        self.account_id
    }

    /// The configured polling interval (used by reconcile's bounded re-reads).
    pub fn poll_interval(&self) -> std::time::Duration {
        self.poll_interval
    }

    pub fn program_id(&self) -> ProgramId {
        self.program_id
    }

    /// Idempotently ensure this client's account is initialized on-chain
    /// (owned by the `authenticated_transfer` program). See
    /// [`crate::lez::onboard::ensure_initialized`] — the sequencer silently
    /// drops claims/transfers against a never-initialized account, so this
    /// must run before either.
    pub async fn ensure_initialized(&self) -> Result<crate::lez::onboard::InitOutcome> {
        let signer = self.as_onboard_signer();
        crate::lez::onboard::ensure_initialized(self.sequencer(), &signer)
            .await
            .map_err(SwapError::LezTransaction)
    }

    /// Claim from the native pinata faucet until this client's account
    /// balance reaches `target`. See
    /// [`crate::lez::onboard::claim_to_target`] for the full semantics
    /// (auto-initializes first, 3s between claims, aborts after 5
    /// consecutive failures).
    pub async fn claim_to_target(
        &self,
        target: u128,
        progress: Option<crate::lez::onboard::FundingProgressSender>,
    ) -> Result<u128> {
        let signer = self.as_onboard_signer();
        crate::lez::onboard::claim_to_target(self.sequencer(), &signer, target, progress)
            .await
            .map_err(SwapError::LezTransaction)
    }

    fn as_onboard_signer(&self) -> crate::lez::onboard::Signer {
        crate::lez::onboard::Signer {
            account_id: self.account_id,
            signing_key: self.private_key().clone(),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    /// Build, sign, and submit an HTLC program instruction.
    async fn send_htlc_instruction(
        &self,
        account_ids: Vec<AccountId>,
        instruction: HTLCInstruction,
    ) -> Result<String> {
        let nonces = self.get_nonces(&[self.account_id]).await?;

        let message = Message::try_new(self.program_id, account_ids, nonces, instruction)
            .map_err(|e| SwapError::LezTransaction(format!("failed to build message: {e}")))?;

        let witness_set = WitnessSet::for_message(&message, &[self.private_key()]);
        let tx = PublicTransaction::new(message, witness_set);

        let tx_hash = self
            .sequencer()
            .send_transaction(LeeTransaction::Public(tx))
            .await
            .map_err(|e| SwapError::LezTransaction(format!("send_transaction failed: {e}")))?;

        Ok(tx_hash.to_string())
    }

    /// Fetch current nonces for the given signer accounts.
    async fn get_nonces(&self, signers: &[AccountId]) -> Result<Vec<Nonce>> {
        let ids: Vec<AccountId> = signers.to_vec();
        let resp = self
            .sequencer()
            .get_accounts_nonces(ids)
            .await
            .map_err(|e| SwapError::LezSequencer(format!("get_accounts_nonces failed: {e}")))?;

        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_program_id() -> ProgramId {
        [1u32, 2, 3, 4, 5, 6, 7, 8]
    }

    // Robustness-gap regression (deployed maker, live-observed sequencer
    // timeouts): a transient sequencer blip that recovers within the retry
    // budget must be entirely invisible to the caller's `Result` — it comes
    // back `Ok`, not a failure that happened to be swallowed. `on_transient`
    // is the ONLY signal a caller gets that a blip occurred, which is what
    // lets `run_maker_loop` bump a separate `transient_errors` counter instead
    // of `failed`.
    #[tokio::test(start_paused = true)]
    async fn balance_retry_recovers_from_transient_errors_without_failing() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let calls = AtomicU32::new(0);
        let transient_hits = AtomicU32::new(0);
        let result = balance_with_retry_core(
            || {
                let n = calls.fetch_add(1, Ordering::Relaxed);
                async move {
                    if n < 2 {
                        // First two attempts: sequencer timeout (transient).
                        Err(SwapError::LezSequencer(
                            "get_account_balance failed: Request timeout".into(),
                        ))
                    } else {
                        // Third attempt recovers.
                        Ok(42u128)
                    }
                }
            },
            |_| {
                transient_hits.fetch_add(1, Ordering::Relaxed);
            },
        )
        .await;

        assert_eq!(
            result.expect("the third attempt succeeds, so the overall read must be Ok"),
            42,
            "a recovered read must return the real balance, not an error"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            3,
            "must have retried past the two transient failures"
        );
        assert_eq!(
            transient_hits.load(Ordering::Relaxed),
            1,
            "on_transient fires exactly once per call, regardless of how many attempts \
             it took to recover — this is the signal a caller uses to bump a \
             transient-errors counter WITHOUT touching a swap-failure counter"
        );
    }

    // The other half of the same regression: when the sequencer is down for
    // the whole retry budget, the read still comes back as an ordinary `Err`
    // (the caller decides what a persistent outage means for ITS counters —
    // this function does not itself distinguish "genuine swap failure" from
    // "infrastructure still down"), but `on_transient` still fires exactly
    // once so a caller can flag it as infra flakiness rather than double-count
    // via both a transient AND a failure signal.
    #[tokio::test(start_paused = true)]
    async fn balance_retry_exhausts_bounded_attempts_and_reports_transient_once() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let calls = AtomicU32::new(0);
        let transient_hits = AtomicU32::new(0);
        let result = balance_with_retry_core(
            || {
                calls.fetch_add(1, Ordering::Relaxed);
                async {
                    Err(SwapError::LezSequencer(
                        "get_account_balance failed: Request timeout".into(),
                    ))
                }
            },
            |_| {
                transient_hits.fetch_add(1, Ordering::Relaxed);
            },
        )
        .await;

        assert!(
            result.is_err(),
            "an outage spanning the whole retry budget must still surface as Err"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            BALANCE_RETRY_ATTEMPTS,
            "must stop at the bounded attempt count, not retry forever"
        );
        assert_eq!(
            transient_hits.load(Ordering::Relaxed),
            1,
            "on_transient still fires exactly once, not once per failed attempt"
        );
    }

    #[test]
    fn pda_derivation_is_deterministic() {
        let program_id = test_program_id();
        let hashlock = [0xABu8; 32];
        let seed = PdaSeed::new(hashlock);

        let pda1 = AccountId::for_public_pda(&program_id, &seed);
        let pda2 = AccountId::for_public_pda(&program_id, &seed);
        assert_eq!(pda1, pda2);
    }

    #[test]
    fn pda_differs_for_different_hashlocks() {
        let program_id = test_program_id();
        let pda_a = AccountId::for_public_pda(&program_id, &PdaSeed::new([0xAAu8; 32]));
        let pda_b = AccountId::for_public_pda(&program_id, &PdaSeed::new([0xBBu8; 32]));
        assert_ne!(pda_a, pda_b);
    }

    // P1-C: a `None` (absent/phantom/short) escrow read is NEVER a confirmed
    // refund — refunds leave the account with `state == Refunded` and intact
    // data. Only an OBSERVED `Refunded` state is terminal; `None` must be
    // retried (surfaced as Unknown), never treated as a completed refund.
    // P1-A suspenders: a fresh lock must refuse to fund whenever an escrow
    // already exists for the hashlock (any state) — otherwise the confirmation
    // poll sees the existing PDA immediately and Step 2 double-transfers.
    #[test]
    fn existing_escrow_refuses_fresh_lock() {
        assert!(!existing_escrow_blocks_fresh_lock(None));
        assert!(existing_escrow_blocks_fresh_lock(Some(HTLCState::Locked)));
        assert!(existing_escrow_blocks_fresh_lock(Some(HTLCState::Claimed)));
        assert!(existing_escrow_blocks_fresh_lock(Some(HTLCState::Refunded)));
    }

    // Principle (i): the lock-confirmation poll only accepts an escrow that is
    // conclusively OUR submitted lock. A pre-existing escrow (different taker,
    // amount, timelock, or terminal state) observed after a phantom `None`
    // pre-check must NOT be treated as confirmation — funding it would strand
    // the transfer.
    #[test]
    fn lock_confirmation_requires_exact_match() {
        let program_id = test_program_id();
        let me = AccountId::for_public_pda(&program_id, &PdaSeed::new([0x00u8; 32]));
        let taker = AccountId::for_public_pda(&program_id, &PdaSeed::new([0x01u8; 32]));
        let other = AccountId::for_public_pda(&program_id, &PdaSeed::new([0x02u8; 32]));
        // Our escrow: WE are the maker.
        let make = |state: HTLCState| HTLCEscrow {
            hashlock: [0xAAu8; 32],
            maker_id: me,
            taker_id: taker,
            amount: 150,
            state,
            timelock: 1_000_000,
            preimage: None,
        };
        let escrow = make(HTLCState::Locked);

        assert!(escrow_confirms_our_lock(&escrow, &me, &taker, 150, 1_000_000));
        // Wrong taker / amount / timelock — someone else's escrow.
        assert!(!escrow_confirms_our_lock(&escrow, &me, &other, 150, 1_000_000));
        assert!(!escrow_confirms_our_lock(&escrow, &me, &taker, 151, 1_000_000));
        assert!(!escrow_confirms_our_lock(&escrow, &me, &taker, 150, 999_999));
        // P1-3: a DIFFERENT maker's escrow with our exact hashlock/taker/amount/
        // timelock must NOT confirm — funding it would land our transfer in an
        // escrow only they can refund.
        let foreign = HTLCEscrow {
            maker_id: other,
            ..make(HTLCState::Locked)
        };
        assert!(
            !escrow_confirms_our_lock(&foreign, &me, &taker, 150, 1_000_000),
            "foreign-maker escrow must never confirm our lock"
        );
        // Terminal states never confirm a fresh lock.
        for state in [HTLCState::Claimed, HTLCState::Refunded] {
            assert!(!escrow_confirms_our_lock(
                &make(state),
                &me,
                &taker,
                150,
                1_000_000
            ));
        }
    }

    #[test]
    fn absent_escrow_read_is_not_a_refund() {
        assert_eq!(classify_refund_read(None), RefundReadClass::Absent);
        assert_ne!(classify_refund_read(None), RefundReadClass::Refunded);
        assert_eq!(
            classify_refund_read(Some(HTLCState::Refunded)),
            RefundReadClass::Refunded
        );
        assert_eq!(
            classify_refund_read(Some(HTLCState::Claimed)),
            RefundReadClass::ClaimedByTaker
        );
        assert_eq!(
            classify_refund_read(Some(HTLCState::Locked)),
            RefundReadClass::StillLocked
        );
    }
}
