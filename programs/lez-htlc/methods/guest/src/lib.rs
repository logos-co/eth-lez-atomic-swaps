use nssa_core::account::AccountId;
use serde::{Deserialize, Serialize};

/// Instructions the HTLC program can execute.
#[derive(Serialize, Deserialize)]
pub enum HTLCInstruction {
    /// Maker locks λ into an escrow PDA.
    Lock {
        /// SHA-256 hash of the secret preimage.
        hashlock: [u8; 32],
        /// Account ID of the taker who can claim with the preimage.
        taker_id: AccountId,
        /// Amount of λ to lock.
        amount: u128,
    },
    /// Taker reveals the preimage to claim the locked λ.
    Claim {
        /// The secret whose SHA-256 hash matches the hashlock.
        preimage: Vec<u8>,
    },
    /// Maker reclaims λ from the escrow.
    /// Timelock is enforced off-chain by the CLI before submitting this instruction.
    Refund,
}

/// Lifecycle states of an HTLC escrow.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum HTLCState {
    Locked = 0,
    Claimed = 1,
    Refunded = 2,
}

/// Data stored in the escrow PDA account.
#[derive(Serialize, Deserialize)]
pub struct HTLCEscrow {
    /// SHA-256 hash of the secret preimage.
    pub hashlock: [u8; 32],
    /// Account ID of the maker (depositor / can refund).
    pub maker_id: AccountId,
    /// Account ID of the taker (can claim with preimage).
    pub taker_id: AccountId,
    /// Amount of λ locked in escrow.
    pub amount: u128,
    /// Current state of the escrow.
    pub state: HTLCState,
    /// Preimage, populated when the taker claims.
    pub preimage: Option<Vec<u8>>,
}

impl HTLCEscrow {
    /// Serialize to bytes for storage in account data.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // hashlock: 32 bytes
        buf.extend_from_slice(&self.hashlock);

        // maker_id: 32 bytes
        buf.extend_from_slice(self.maker_id.value());

        // taker_id: 32 bytes
        buf.extend_from_slice(self.taker_id.value());

        // amount: 16 bytes (little-endian)
        buf.extend_from_slice(&self.amount.to_le_bytes());

        // state: 1 byte
        buf.push(self.state as u8);

        // preimage: 4 bytes length prefix + data
        match &self.preimage {
            Some(p) => {
                buf.extend_from_slice(&(p.len() as u32).to_le_bytes());
                buf.extend_from_slice(p);
            }
            None => {
                buf.extend_from_slice(&0u32.to_le_bytes());
            }
        }

        buf
    }

    /// Deserialize from bytes stored in account data.
    pub fn from_bytes(data: &[u8]) -> Self {
        // Minimum size: 32 + 32 + 32 + 16 + 1 + 4 = 117 bytes
        assert!(data.len() >= 117, "escrow data too short");

        let hashlock: [u8; 32] = data[0..32].try_into().unwrap();
        let maker_id = AccountId::new(data[32..64].try_into().unwrap());
        let taker_id = AccountId::new(data[64..96].try_into().unwrap());
        let amount = u128::from_le_bytes(data[96..112].try_into().unwrap());
        let state = match data[112] {
            0 => HTLCState::Locked,
            1 => HTLCState::Claimed,
            2 => HTLCState::Refunded,
            s => panic!("invalid escrow state: {s}"),
        };

        let preimage_len = u32::from_le_bytes(data[113..117].try_into().unwrap()) as usize;
        let preimage = if preimage_len > 0 {
            assert!(
                data.len() >= 117 + preimage_len,
                "escrow data truncated: expected preimage"
            );
            Some(data[117..117 + preimage_len].to_vec())
        } else {
            None
        };

        Self {
            hashlock,
            maker_id,
            taker_id,
            amount,
            state,
            preimage,
        }
    }
}
