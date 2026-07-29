use lez_htlc_program::{HTLCEscrow, HTLCInstruction, HTLCState};
use lee::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use lee_core::{
    account::Nonce,
    program::{PdaSeed, ProgramId},
};
use sequencer_service_protocol::LeeTransaction;
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use tracing::{debug, info};
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
        let instruction = HTLCInstruction::Lock {
            hashlock,
            taker_id,
            amount,
            timelock: timelock_secs * 1000,
        };

        let lock_hash = self
            .send_htlc_instruction(vec![self.account_id, pda], instruction)
            .await?;
        debug!(tx_hash = %lock_hash, "LEZ HTLC lock submitted");

        // Wait for the lock to be committed before funding. Public-testnet
        // blocks can be a minute or more apart, so allow several blocks.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if self.get_escrow(&hashlock).await?.is_some() {
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

    pub fn program_id(&self) -> ProgramId {
        self.program_id
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
