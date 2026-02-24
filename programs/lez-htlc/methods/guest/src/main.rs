#![cfg_attr(not(test), no_main)]

#[cfg(not(test))]
use lez_htlc_program::HTLCInstruction;
use lez_htlc_program::{HTLCEscrow, HTLCState};
use nssa_core::account::{AccountId, AccountWithMetadata};
use nssa_core::program::AccountPostState;
use lez_framework::prelude::*;
use risc0_zkvm::sha::{Impl, Sha256};

#[cfg(not(test))]
risc0_zkvm::guest::entry!(main);

#[cfg(not(test))]
#[lez_program(instruction = "HTLCInstruction")]
mod lez_htlc {
    use super::*;

    #[instruction]
    pub fn lock(
        #[account(signer)]
        maker: AccountWithMetadata,
        #[account(init, pda = arg("hashlock"))]
        escrow: AccountWithMetadata,
        hashlock: [u8; 32],
        taker_id: AccountId,
        amount: u128,
    ) -> LezResult {
        lock_impl(maker, escrow, hashlock, taker_id, amount)
    }

    #[instruction]
    pub fn claim(
        #[account(signer)]
        taker: AccountWithMetadata,
        #[account(mut, pda = arg("hashlock"))]
        escrow: AccountWithMetadata,
        hashlock: [u8; 32],
        preimage: Vec<u8>,
    ) -> LezResult {
        claim_impl(taker, escrow, hashlock, preimage)
    }

    #[instruction]
    pub fn refund(
        #[account(signer)]
        maker: AccountWithMetadata,
        #[account(mut, pda = arg("hashlock"))]
        escrow: AccountWithMetadata,
        hashlock: [u8; 32],
    ) -> LezResult {
        refund_impl(maker, escrow, hashlock)
    }
}

// Domain logic extracted so it's testable on host (without risc0 guest env).
// The #[lez_program] handlers above delegate to these functions.

fn lock_impl(
    maker: AccountWithMetadata,
    escrow: AccountWithMetadata,
    hashlock: [u8; 32],
    taker_id: AccountId,
    amount: u128,
) -> LezResult {
    assert!(maker.account_id != taker_id, "maker and taker must differ");

    let escrow_data = HTLCEscrow {
        hashlock,
        maker_id: maker.account_id,
        taker_id,
        amount,
        state: HTLCState::Locked,
        preimage: None,
    };

    let mut escrow_account = escrow.account.clone();
    escrow_account.data = escrow_data
        .to_bytes()
        .try_into()
        .expect("escrow data fits in Data");

    Ok(LezOutput::states_only(vec![
        AccountPostState::new(maker.account.clone()),
        AccountPostState::new_claimed(escrow_account),
    ]))
}

fn claim_impl(
    taker: AccountWithMetadata,
    escrow: AccountWithMetadata,
    _hashlock: [u8; 32],
    preimage: Vec<u8>,
) -> LezResult {
    assert!(preimage.len() == 32, "preimage must be exactly 32 bytes");

    let mut escrow_data = HTLCEscrow::from_bytes(&escrow.account.data);
    assert!(escrow_data.state == HTLCState::Locked, "escrow must be Locked");
    assert!(
        taker.account_id == escrow_data.taker_id,
        "only designated taker can claim"
    );

    // Verify SHA-256(preimage) == hashlock
    let computed: [u8; 32] = Impl::hash_bytes(&preimage).as_bytes().try_into().unwrap();
    assert!(computed == escrow_data.hashlock, "invalid preimage");

    // Transfer from escrow to taker
    let mut taker_account = taker.account.clone();
    let mut escrow_account = escrow.account.clone();
    assert!(
        escrow_account.balance >= escrow_data.amount,
        "escrow balance insufficient for claim"
    );
    escrow_account.balance -= escrow_data.amount;
    taker_account.balance += escrow_data.amount;

    // Update escrow state
    escrow_data.state = HTLCState::Claimed;
    escrow_data.preimage = Some(preimage.to_vec());
    escrow_account.data = escrow_data
        .to_bytes()
        .try_into()
        .expect("escrow data fits in Data");

    Ok(LezOutput::states_only(vec![
        AccountPostState::new(taker_account),
        AccountPostState::new(escrow_account),
    ]))
}

fn refund_impl(
    maker: AccountWithMetadata,
    escrow: AccountWithMetadata,
    _hashlock: [u8; 32],
) -> LezResult {
    let mut escrow_data = HTLCEscrow::from_bytes(&escrow.account.data);
    assert!(escrow_data.state == HTLCState::Locked, "escrow must be Locked");
    assert!(
        maker.account_id == escrow_data.maker_id,
        "only maker can refund"
    );

    // Transfer from escrow back to maker
    let mut maker_account = maker.account.clone();
    let mut escrow_account = escrow.account.clone();
    assert!(
        escrow_account.balance >= escrow_data.amount,
        "escrow balance insufficient for refund"
    );
    escrow_account.balance -= escrow_data.amount;
    maker_account.balance += escrow_data.amount;

    // Update escrow state
    escrow_data.state = HTLCState::Refunded;
    escrow_account.data = escrow_data
        .to_bytes()
        .try_into()
        .expect("escrow data fits in Data");

    Ok(LezOutput::states_only(vec![
        AccountPostState::new(maker_account),
        AccountPostState::new(escrow_account),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lez_htlc_program::{HTLCEscrow, HTLCState};
    use nssa_core::account::{Account, AccountId, AccountWithMetadata};
    use nssa_core::program::DEFAULT_PROGRAM_ID;
    use risc0_zkvm::sha::{Impl, Sha256};

    const AMOUNT: u128 = 1_000;
    const SECRET: &[u8; 32] = b"supersecretpreimage_padding_0123";
    const PROGRAM_ID: [u32; 8] = [5; 8];

    fn maker_id() -> AccountId {
        AccountId::new([1u8; 32])
    }
    fn taker_id() -> AccountId {
        AccountId::new([2u8; 32])
    }
    fn wrong_id() -> AccountId {
        AccountId::new([99u8; 32])
    }

    fn hashlock() -> [u8; 32] {
        Impl::hash_bytes(SECRET).as_bytes().try_into().unwrap()
    }

    fn locked_escrow_data() -> Vec<u8> {
        HTLCEscrow {
            hashlock: hashlock(),
            maker_id: maker_id(),
            taker_id: taker_id(),
            amount: AMOUNT,
            state: HTLCState::Locked,
            preimage: None,
        }
        .to_bytes()
    }

    fn escrow_data_with_state(state: HTLCState) -> Vec<u8> {
        HTLCEscrow {
            hashlock: hashlock(),
            maker_id: maker_id(),
            taker_id: taker_id(),
            amount: AMOUNT,
            state,
            preimage: if state == HTLCState::Claimed {
                Some(SECRET.to_vec())
            } else {
                None
            },
        }
        .to_bytes()
    }

    fn maker_account() -> AccountWithMetadata {
        AccountWithMetadata {
            account: Account {
                program_owner: DEFAULT_PROGRAM_ID,
                balance: 0,
                data: Default::default(),
                nonce: 0,
            },
            is_authorized: true,
            account_id: maker_id(),
        }
    }

    fn uninit_escrow() -> AccountWithMetadata {
        AccountWithMetadata {
            account: Account::default(),
            is_authorized: false,
            account_id: AccountId::new([0xEE; 32]),
        }
    }

    fn taker_account() -> AccountWithMetadata {
        AccountWithMetadata {
            account: Account {
                program_owner: DEFAULT_PROGRAM_ID,
                balance: 500,
                data: Default::default(),
                nonce: 0,
            },
            is_authorized: true,
            account_id: taker_id(),
        }
    }

    fn locked_escrow() -> AccountWithMetadata {
        AccountWithMetadata {
            account: Account {
                program_owner: PROGRAM_ID,
                balance: AMOUNT,
                data: locked_escrow_data().try_into().expect("escrow data fits"),
                nonce: 0,
            },
            is_authorized: false,
            account_id: AccountId::new([0xEE; 32]),
        }
    }

    fn maker_account_with_balance() -> AccountWithMetadata {
        AccountWithMetadata {
            account: Account {
                program_owner: DEFAULT_PROGRAM_ID,
                balance: 500,
                data: Default::default(),
                nonce: 0,
            },
            is_authorized: true,
            account_id: maker_id(),
        }
    }

    // ── Lock tests ──────────────────────────────────────────────────

    #[test]
    fn test_lock_happy_path() {
        let maker = maker_account();
        let escrow = uninit_escrow();
        let result = lock_impl(maker.clone(), escrow, hashlock(), taker_id(), AMOUNT).unwrap();

        // Maker account unchanged
        assert_eq!(result.post_states[0].account().balance, maker.account.balance);
        assert!(!result.post_states[0].requires_claim());

        // Escrow PDA claimed by program, data populated
        assert!(result.post_states[1].requires_claim());
        let escrow_data = HTLCEscrow::from_bytes(&result.post_states[1].account().data);
        assert_eq!(escrow_data.hashlock, hashlock());
        assert_eq!(escrow_data.maker_id, maker_id());
        assert_eq!(escrow_data.taker_id, taker_id());
        assert_eq!(escrow_data.amount, AMOUNT);
        assert_eq!(escrow_data.state, HTLCState::Locked);
        assert_eq!(escrow_data.preimage, None);
    }

    #[test]
    #[should_panic(expected = "maker and taker must differ")]
    fn test_lock_self_swap() {
        let maker = maker_account();
        let escrow = uninit_escrow();
        let _ = lock_impl(maker, escrow, hashlock(), maker_id(), AMOUNT);
    }

    // ── Claim tests ─────────────────────────────────────────────────

    #[test]
    fn test_claim_happy_path() {
        let taker = taker_account();
        let escrow = locked_escrow();
        let result = claim_impl(taker, escrow, hashlock(), SECRET.to_vec()).unwrap();

        // Taker received funds
        assert_eq!(result.post_states[0].account().balance, 500 + AMOUNT);
        assert!(!result.post_states[0].requires_claim());

        // Escrow drained, state updated
        assert_eq!(result.post_states[1].account().balance, 0);
        assert!(!result.post_states[1].requires_claim());
        let escrow_data = HTLCEscrow::from_bytes(&result.post_states[1].account().data);
        assert_eq!(escrow_data.state, HTLCState::Claimed);
        assert_eq!(escrow_data.preimage, Some(SECRET.to_vec()));
    }

    #[test]
    #[should_panic(expected = "invalid preimage")]
    fn test_claim_wrong_preimage() {
        let taker = taker_account();
        let escrow = locked_escrow();
        let _ = claim_impl(taker, escrow, hashlock(), b"wrong_secret_preimage_padding_01".to_vec());
    }

    #[test]
    #[should_panic(expected = "preimage must be exactly 32 bytes")]
    fn test_claim_wrong_preimage_length() {
        let taker = taker_account();
        let escrow = locked_escrow();
        let _ = claim_impl(taker, escrow, hashlock(), b"too_short".to_vec());
    }

    #[test]
    #[should_panic(expected = "only designated taker can claim")]
    fn test_claim_wrong_taker() {
        let mut taker = taker_account();
        taker.account_id = wrong_id();
        let escrow = locked_escrow();
        let _ = claim_impl(taker, escrow, hashlock(), SECRET.to_vec());
    }

    #[test]
    #[should_panic(expected = "escrow must be Locked")]
    fn test_claim_already_claimed() {
        let taker = taker_account();
        let mut escrow = locked_escrow();
        escrow.account.data = escrow_data_with_state(HTLCState::Claimed)
            .try_into()
            .expect("fits");
        let _ = claim_impl(taker, escrow, hashlock(), SECRET.to_vec());
    }

    #[test]
    #[should_panic(expected = "escrow must be Locked")]
    fn test_claim_already_refunded() {
        let taker = taker_account();
        let mut escrow = locked_escrow();
        escrow.account.data = escrow_data_with_state(HTLCState::Refunded)
            .try_into()
            .expect("fits");
        let _ = claim_impl(taker, escrow, hashlock(), SECRET.to_vec());
    }

    // ── Refund tests ────────────────────────────────────────────────

    #[test]
    fn test_refund_happy_path() {
        let maker = maker_account_with_balance();
        let escrow = locked_escrow();
        let result = refund_impl(maker, escrow, hashlock()).unwrap();

        // Maker received funds back
        assert_eq!(result.post_states[0].account().balance, 500 + AMOUNT);
        assert!(!result.post_states[0].requires_claim());

        // Escrow drained, state updated
        assert_eq!(result.post_states[1].account().balance, 0);
        assert!(!result.post_states[1].requires_claim());
        let escrow_data = HTLCEscrow::from_bytes(&result.post_states[1].account().data);
        assert_eq!(escrow_data.state, HTLCState::Refunded);
    }

    #[test]
    #[should_panic(expected = "only maker can refund")]
    fn test_refund_wrong_maker() {
        let mut maker = maker_account_with_balance();
        maker.account_id = wrong_id();
        let escrow = locked_escrow();
        let _ = refund_impl(maker, escrow, hashlock());
    }

    #[test]
    #[should_panic(expected = "escrow must be Locked")]
    fn test_refund_already_claimed() {
        let maker = maker_account_with_balance();
        let mut escrow = locked_escrow();
        escrow.account.data = escrow_data_with_state(HTLCState::Claimed)
            .try_into()
            .expect("fits");
        let _ = refund_impl(maker, escrow, hashlock());
    }

    #[test]
    #[should_panic(expected = "escrow must be Locked")]
    fn test_refund_already_refunded() {
        let maker = maker_account_with_balance();
        let mut escrow = locked_escrow();
        escrow.account.data = escrow_data_with_state(HTLCState::Refunded)
            .try_into()
            .expect("fits");
        let _ = refund_impl(maker, escrow, hashlock());
    }

    // ── Cross-chain compatibility tests ─────────────────────────────
    const XCHAIN_PREIMAGE: &[u8; 32] = b"secret_preimage_for_testing_1234";
    const XCHAIN_HASHLOCK: [u8; 32] = [
        0x0e, 0xf6, 0x96, 0x11, 0xa9, 0x1e, 0x08, 0x05,
        0x07, 0x93, 0x87, 0xfe, 0xe0, 0xb8, 0x9f, 0xb7,
        0xd6, 0xfc, 0xd5, 0x05, 0x22, 0x0d, 0x40, 0x7b,
        0xac, 0xaa, 0x40, 0xce, 0x03, 0x17, 0x45, 0xdf,
    ];

    #[test]
    fn test_crosschain_sha256_compatibility() {
        let computed: [u8; 32] = Impl::hash_bytes(XCHAIN_PREIMAGE)
            .as_bytes()
            .try_into()
            .unwrap();
        assert_eq!(computed, XCHAIN_HASHLOCK);
    }

    #[test]
    fn test_crosschain_lock_then_claim_with_shared_preimage() {
        let maker = AccountWithMetadata {
            account: Account {
                program_owner: DEFAULT_PROGRAM_ID,
                balance: 0,
                data: Default::default(),
                nonce: 0,
            },
            is_authorized: true,
            account_id: maker_id(),
        };
        let escrow = AccountWithMetadata {
            account: Account::default(),
            is_authorized: false,
            account_id: AccountId::new([0xEE; 32]),
        };

        let lock_result = lock_impl(maker, escrow, XCHAIN_HASHLOCK, taker_id(), AMOUNT).unwrap();
        assert!(lock_result.post_states[1].requires_claim());

        // Simulate the transfer that happens after Lock (funds the PDA).
        let mut funded_escrow = lock_result.post_states[1].account().clone();
        funded_escrow.balance = AMOUNT;

        // Now simulate claim: taker uses the preimage revealed on Ethereum.
        let taker = AccountWithMetadata {
            account: Account {
                program_owner: DEFAULT_PROGRAM_ID,
                balance: 500,
                data: Default::default(),
                nonce: 0,
            },
            is_authorized: true,
            account_id: taker_id(),
        };
        let escrow_for_claim = AccountWithMetadata {
            account: funded_escrow,
            is_authorized: false,
            account_id: AccountId::new([0xEE; 32]),
        };

        let claim_result = claim_impl(taker, escrow_for_claim, XCHAIN_HASHLOCK, XCHAIN_PREIMAGE.to_vec()).unwrap();

        assert_eq!(claim_result.post_states[0].account().balance, 500 + AMOUNT);
        let escrow_data = HTLCEscrow::from_bytes(&claim_result.post_states[1].account().data);
        assert_eq!(escrow_data.state, HTLCState::Claimed);
        assert_eq!(escrow_data.preimage, Some(XCHAIN_PREIMAGE.to_vec()));
    }

    #[test]
    fn test_crosschain_refund_after_timeout() {
        let maker = AccountWithMetadata {
            account: Account {
                program_owner: DEFAULT_PROGRAM_ID,
                balance: 500,
                data: Default::default(),
                nonce: 0,
            },
            is_authorized: true,
            account_id: maker_id(),
        };
        let escrow = AccountWithMetadata {
            account: Account::default(),
            is_authorized: false,
            account_id: AccountId::new([0xEE; 32]),
        };

        let lock_result = lock_impl(maker, escrow, XCHAIN_HASHLOCK, taker_id(), AMOUNT).unwrap();

        // Simulate the transfer that happens after Lock (funds the PDA).
        let mut funded_escrow = lock_result.post_states[1].account().clone();
        funded_escrow.balance = AMOUNT;

        // Maker refunds (CLI enforced timelock off-chain)
        let maker_for_refund = AccountWithMetadata {
            account: Account {
                program_owner: DEFAULT_PROGRAM_ID,
                balance: 500,
                data: Default::default(),
                nonce: 0,
            },
            is_authorized: true,
            account_id: maker_id(),
        };
        let escrow_for_refund = AccountWithMetadata {
            account: funded_escrow,
            is_authorized: false,
            account_id: AccountId::new([0xEE; 32]),
        };

        let refund_result = refund_impl(maker_for_refund, escrow_for_refund, XCHAIN_HASHLOCK).unwrap();

        assert_eq!(refund_result.post_states[0].account().balance, 500 + AMOUNT);
        let escrow_data = HTLCEscrow::from_bytes(&refund_result.post_states[1].account().data);
        assert_eq!(escrow_data.state, HTLCState::Refunded);
    }
}
