use common::sequencer_client::SequencerClient;
use lez_htlc_program::{HTLCEscrow, HTLCInstruction};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use nssa_core::program::{PdaSeed, ProgramId};
use tracing::{debug, info};
use url::Url;

use crate::{
    config::SwapConfig,
    error::{Result, SwapError},
};

pub struct LezClient {
    sequencer: SequencerClient,
    private_key: PrivateKey,
    account_id: AccountId,
    program_id: ProgramId,
    poll_interval: std::time::Duration,
}

impl LezClient {
    pub fn new(config: &SwapConfig) -> Result<Self> {
        let key_bytes: [u8; 32] = hex::decode(&config.lez_signing_key)
            .map_err(|e| SwapError::InvalidConfig(format!("invalid LEZ signing key hex: {e}")))?
            .try_into()
            .map_err(|_| SwapError::InvalidConfig("LEZ signing key must be 32 bytes".into()))?;

        let private_key = PrivateKey::try_new(key_bytes)
            .map_err(|e| SwapError::InvalidConfig(format!("invalid LEZ private key: {e}")))?;

        let public_key = PublicKey::new_from_private_key(&private_key);
        let account_id = AccountId::from(&public_key);

        let sequencer_url = Url::parse(&config.lez_sequencer_url)
            .map_err(|e| SwapError::InvalidConfig(format!("invalid sequencer URL: {e}")))?;

        let sequencer = SequencerClient::new(sequencer_url)
            .map_err(|e| SwapError::LezSequencer(format!("failed to create client: {e}")))?;

        Ok(Self {
            sequencer,
            private_key,
            account_id,
            program_id: config.lez_htlc_program_id,
            poll_interval: config.poll_interval,
        })
    }

    /// Derive the escrow PDA account ID from a hashlock.
    pub fn escrow_pda(&self, hashlock: &[u8; 32]) -> AccountId {
        AccountId::from((&self.program_id, &PdaSeed::new(*hashlock)))
    }

    /// Read the escrow PDA state. Returns `None` if the account has no data.
    pub async fn get_escrow(&self, hashlock: &[u8; 32]) -> Result<Option<HTLCEscrow>> {
        let pda = self.escrow_pda(hashlock);
        let resp = self
            .sequencer
            .get_account(pda.to_string())
            .await
            .map_err(|e| SwapError::LezSequencer(format!("get_account failed: {e}")))?;

        let data: Vec<u8> = resp.account.data.into();
        if data.is_empty() {
            return Ok(None);
        }

        let escrow = HTLCEscrow::from_bytes(&data);
        Ok(Some(escrow))
    }

    /// Read the balance of an account.
    pub async fn get_balance(&self, account_id: &AccountId) -> Result<u128> {
        let resp = self
            .sequencer
            .get_account_balance(account_id.to_string())
            .await
            .map_err(|e| SwapError::LezSequencer(format!("get_account_balance failed: {e}")))?;

        Ok(resp.balance)
    }

    /// Transfer LEZ to a recipient using the authenticated transfer program.
    pub async fn transfer(&self, recipient: AccountId, amount: u128) -> Result<String> {
        let program_id = Program::authenticated_transfer_program().id();
        let account_ids = vec![self.account_id, recipient];

        let nonces = self.get_nonces(&[self.account_id]).await?;

        let message = Message::try_new(program_id, account_ids, nonces, amount)
            .map_err(|e| SwapError::LezTransaction(format!("failed to build message: {e}")))?;

        let witness_set = WitnessSet::for_message(&message, &[&self.private_key]);
        let tx = PublicTransaction::new(message, witness_set);

        let resp = self
            .sequencer
            .send_tx_public(tx)
            .await
            .map_err(|e| SwapError::LezTransaction(format!("transfer failed: {e}")))?;

        info!(tx_hash = %resp.tx_hash, amount, "LEZ transfer submitted");
        Ok(resp.tx_hash)
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
    ) -> Result<String> {
        let pda = self.escrow_pda(&hashlock);

        // Step 1: Lock — claims the uninitialized PDA and stores escrow data.
        let instruction = HTLCInstruction::Lock {
            hashlock,
            taker_id,
            amount,
        };

        let lock_hash = self
            .send_htlc_instruction(
                vec![self.account_id, pda],
                instruction,
            )
            .await?;
        debug!(tx_hash = %lock_hash, "LEZ HTLC lock submitted");

        // Wait for the lock to be committed before funding.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
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
        debug!(tx_hash = %transfer_hash, "escrow PDA funded");

        // Wait for the transfer to be committed.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let balance = self.get_balance(&pda).await?;
            if balance >= amount {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(SwapError::Timeout("LEZ escrow funding confirmation".into()));
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
            .send_htlc_instruction(
                vec![self.account_id, pda],
                HTLCInstruction::Refund,
            )
            .await?;

        info!(tx_hash = %tx_hash, "LEZ HTLC refunded");
        Ok(tx_hash)
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

        let witness_set = WitnessSet::for_message(&message, &[&self.private_key]);
        let tx = PublicTransaction::new(message, witness_set);

        let resp = self
            .sequencer
            .send_tx_public(tx)
            .await
            .map_err(|e| SwapError::LezTransaction(format!("send_tx_public failed: {e}")))?;

        Ok(resp.tx_hash)
    }

    /// Fetch current nonces for the given signer accounts.
    async fn get_nonces(&self, signers: &[AccountId]) -> Result<Vec<u128>> {
        let ids: Vec<String> = signers.iter().map(|id| id.to_string()).collect();
        let resp = self
            .sequencer
            .get_accounts_nonces(ids)
            .await
            .map_err(|e| SwapError::LezSequencer(format!("get_accounts_nonces failed: {e}")))?;

        Ok(resp.nonces)
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

        let pda1 = AccountId::from((&program_id, &seed));
        let pda2 = AccountId::from((&program_id, &seed));
        assert_eq!(pda1, pda2);
    }

    #[test]
    fn pda_differs_for_different_hashlocks() {
        let program_id = test_program_id();
        let pda_a = AccountId::from((&program_id, &PdaSeed::new([0xAAu8; 32])));
        let pda_b = AccountId::from((&program_id, &PdaSeed::new([0xBBu8; 32])));
        assert_ne!(pda_a, pda_b);
    }
}
