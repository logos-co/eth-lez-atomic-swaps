use lez_htlc_program::{HTLCEscrow, HTLCInstruction, HTLCState};
use nssa_core::{
    account::{AccountId, AccountWithMetadata},
    program::{
        read_nssa_inputs, write_nssa_outputs, AccountPostState, ProgramInput, DEFAULT_PROGRAM_ID,
    },
};
use risc0_zkvm::sha::{Impl, Sha256};

fn main() {
    let (
        ProgramInput {
            pre_states,
            instruction,
        },
        instruction_data,
    ) = read_nssa_inputs::<HTLCInstruction>();

    let post_states = match instruction {
        HTLCInstruction::Lock {
            hashlock,
            taker_id,
            amount,
        } => execute_lock(&pre_states, hashlock, taker_id, amount),
        HTLCInstruction::Claim { preimage } => execute_claim(&pre_states, &preimage),
        HTLCInstruction::Refund => execute_refund(&pre_states),
    };

    write_nssa_outputs(instruction_data, pre_states, post_states);
}

fn execute_lock(
    pre_states: &[AccountWithMetadata],
    hashlock: [u8; 32],
    taker_id: AccountId,
    amount: u128,
) -> Vec<AccountPostState> {
    assert!(pre_states.len() == 2, "lock requires 2 accounts: [maker, escrow]");
    let maker = &pre_states[0];
    let escrow_pda = &pre_states[1];

    assert!(maker.is_authorized, "maker must be authorized");
    assert!(
        escrow_pda.account.program_owner == DEFAULT_PROGRAM_ID,
        "escrow PDA must be unclaimed"
    );
    assert!(
        escrow_pda.account.balance == amount,
        "escrow PDA balance must exactly match lock amount"
    );

    let escrow = HTLCEscrow {
        hashlock,
        maker_id: maker.account_id,
        taker_id,
        amount,
        state: HTLCState::Locked,
        preimage: None,
    };

    let mut escrow_account = escrow_pda.account.clone();
    escrow_account.data = escrow
        .to_bytes()
        .try_into()
        .expect("escrow data fits in Data");

    vec![
        AccountPostState::new(maker.account.clone()),
        AccountPostState::new_claimed(escrow_account),
    ]
}

fn execute_claim(
    pre_states: &[AccountWithMetadata],
    preimage: &[u8],
) -> Vec<AccountPostState> {
    assert!(pre_states.len() == 2, "claim requires 2 accounts: [taker, escrow]");
    let taker = &pre_states[0];
    let escrow_pda = &pre_states[1];

    assert!(taker.is_authorized, "taker must be authorized");

    let mut escrow = HTLCEscrow::from_bytes(&escrow_pda.account.data);
    assert!(escrow.state == HTLCState::Locked, "escrow must be Locked");
    assert!(
        taker.account_id == escrow.taker_id,
        "only designated taker can claim"
    );

    // Verify SHA-256(preimage) == hashlock
    let computed: [u8; 32] = Impl::hash_bytes(preimage)
        .as_bytes()
        .try_into()
        .unwrap();
    assert!(computed == escrow.hashlock, "invalid preimage");

    // Transfer from escrow to taker
    let mut taker_account = taker.account.clone();
    let mut escrow_account = escrow_pda.account.clone();
    assert!(
        escrow_account.balance >= escrow.amount,
        "escrow balance insufficient for claim"
    );
    escrow_account.balance -= escrow.amount;
    taker_account.balance += escrow.amount;

    // Update escrow state
    escrow.state = HTLCState::Claimed;
    escrow.preimage = Some(preimage.to_vec());
    escrow_account.data = escrow
        .to_bytes()
        .try_into()
        .expect("escrow data fits in Data");

    vec![
        AccountPostState::new(taker_account),
        AccountPostState::new(escrow_account),
    ]
}

fn execute_refund(pre_states: &[AccountWithMetadata]) -> Vec<AccountPostState> {
    assert!(pre_states.len() == 2, "refund requires 2 accounts: [maker, escrow]");
    let maker = &pre_states[0];
    let escrow_pda = &pre_states[1];

    assert!(maker.is_authorized, "maker must be authorized");

    let mut escrow = HTLCEscrow::from_bytes(&escrow_pda.account.data);
    assert!(escrow.state == HTLCState::Locked, "escrow must be Locked");
    assert!(
        maker.account_id == escrow.maker_id,
        "only maker can refund"
    );

    // Transfer from escrow back to maker
    let mut maker_account = maker.account.clone();
    let mut escrow_account = escrow_pda.account.clone();
    assert!(
        escrow_account.balance >= escrow.amount,
        "escrow balance insufficient for refund"
    );
    escrow_account.balance -= escrow.amount;
    maker_account.balance += escrow.amount;

    // Update escrow state
    escrow.state = HTLCState::Refunded;
    escrow_account.data = escrow
        .to_bytes()
        .try_into()
        .expect("escrow data fits in Data");

    vec![
        AccountPostState::new(maker_account),
        AccountPostState::new(escrow_account),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lez_htlc_program::{HTLCEscrow, HTLCState};
    use nssa_core::account::{Account, AccountId, AccountWithMetadata};
    use nssa_core::program::DEFAULT_PROGRAM_ID;
    use risc0_zkvm::sha::{Impl, Sha256};

    const AMOUNT: u128 = 1_000;
    const SECRET: &[u8] = b"supersecretpreimage";
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

    /// Build pre-states for Lock: [maker, empty escrow PDA (pre-funded)]
    fn lock_pre_states() -> Vec<AccountWithMetadata> {
        vec![
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: 0,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: true,
                account_id: maker_id(),
            },
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: AMOUNT,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: false,
                account_id: AccountId::new([0xEE; 32]),
            },
        ]
    }

    /// Build pre-states for Claim: [taker, locked escrow PDA (program-owned)]
    fn claim_pre_states() -> Vec<AccountWithMetadata> {
        vec![
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: 500,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: true,
                account_id: taker_id(),
            },
            AccountWithMetadata {
                account: Account {
                    program_owner: PROGRAM_ID,
                    balance: AMOUNT,
                    data: locked_escrow_data()
                        .try_into()
                        .expect("escrow data fits"),
                    nonce: 0,
                },
                is_authorized: false,
                account_id: AccountId::new([0xEE; 32]),
            },
        ]
    }

    /// Build pre-states for Refund: [maker, locked escrow PDA (program-owned)]
    fn refund_pre_states() -> Vec<AccountWithMetadata> {
        vec![
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: 500,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: true,
                account_id: maker_id(),
            },
            AccountWithMetadata {
                account: Account {
                    program_owner: PROGRAM_ID,
                    balance: AMOUNT,
                    data: locked_escrow_data()
                        .try_into()
                        .expect("escrow data fits"),
                    nonce: 0,
                },
                is_authorized: false,
                account_id: AccountId::new([0xEE; 32]),
            },
        ]
    }

    // ── Lock tests ──────────────────────────────────────────────────

    #[test]
    fn test_lock_happy_path() {
        let pre = lock_pre_states();
        let post = execute_lock(&pre, hashlock(), taker_id(), AMOUNT);

        // Maker account unchanged
        assert_eq!(post[0].account().balance, pre[0].account.balance);
        assert!(!post[0].requires_claim());

        // Escrow PDA claimed by program, data populated
        assert!(post[1].requires_claim());
        let escrow = HTLCEscrow::from_bytes(&post[1].account().data);
        assert_eq!(escrow.hashlock, hashlock());
        assert_eq!(escrow.maker_id, maker_id());
        assert_eq!(escrow.taker_id, taker_id());
        assert_eq!(escrow.amount, AMOUNT);
        assert_eq!(escrow.state, HTLCState::Locked);
        assert_eq!(escrow.preimage, None);
    }

    #[test]
    #[should_panic(expected = "maker must be authorized")]
    fn test_lock_unauthorized_maker() {
        let mut pre = lock_pre_states();
        pre[0].is_authorized = false;
        execute_lock(&pre, hashlock(), taker_id(), AMOUNT);
    }

    #[test]
    #[should_panic(expected = "escrow PDA must be unclaimed")]
    fn test_lock_escrow_already_owned() {
        let mut pre = lock_pre_states();
        pre[1].account.program_owner = PROGRAM_ID;
        execute_lock(&pre, hashlock(), taker_id(), AMOUNT);
    }

    #[test]
    #[should_panic(expected = "escrow PDA balance must exactly match lock amount")]
    fn test_lock_insufficient_balance() {
        let mut pre = lock_pre_states();
        pre[1].account.balance = AMOUNT - 1;
        execute_lock(&pre, hashlock(), taker_id(), AMOUNT);
    }

    #[test]
    #[should_panic(expected = "escrow PDA balance must exactly match lock amount")]
    fn test_lock_overfunded_balance() {
        let mut pre = lock_pre_states();
        pre[1].account.balance = AMOUNT + 1;
        execute_lock(&pre, hashlock(), taker_id(), AMOUNT);
    }

    // ── Claim tests ─────────────────────────────────────────────────

    #[test]
    fn test_claim_happy_path() {
        let pre = claim_pre_states();
        let post = execute_claim(&pre, SECRET);

        // Taker received funds
        assert_eq!(post[0].account().balance, 500 + AMOUNT);
        assert!(!post[0].requires_claim());

        // Escrow drained, state updated
        assert_eq!(post[1].account().balance, 0);
        assert!(!post[1].requires_claim());
        let escrow = HTLCEscrow::from_bytes(&post[1].account().data);
        assert_eq!(escrow.state, HTLCState::Claimed);
        assert_eq!(escrow.preimage, Some(SECRET.to_vec()));
    }

    #[test]
    #[should_panic(expected = "invalid preimage")]
    fn test_claim_wrong_preimage() {
        let pre = claim_pre_states();
        execute_claim(&pre, b"wrongsecret");
    }

    #[test]
    #[should_panic(expected = "only designated taker can claim")]
    fn test_claim_wrong_taker() {
        let mut pre = claim_pre_states();
        pre[0].account_id = wrong_id();
        execute_claim(&pre, SECRET);
    }

    #[test]
    #[should_panic(expected = "taker must be authorized")]
    fn test_claim_not_authorized() {
        let mut pre = claim_pre_states();
        pre[0].is_authorized = false;
        execute_claim(&pre, SECRET);
    }

    #[test]
    #[should_panic(expected = "escrow must be Locked")]
    fn test_claim_already_claimed() {
        let mut pre = claim_pre_states();
        pre[1].account.data = escrow_data_with_state(HTLCState::Claimed)
            .try_into()
            .expect("fits");
        execute_claim(&pre, SECRET);
    }

    #[test]
    #[should_panic(expected = "escrow must be Locked")]
    fn test_claim_already_refunded() {
        let mut pre = claim_pre_states();
        pre[1].account.data = escrow_data_with_state(HTLCState::Refunded)
            .try_into()
            .expect("fits");
        execute_claim(&pre, SECRET);
    }

    // ── Refund tests ────────────────────────────────────────────────

    #[test]
    fn test_refund_happy_path() {
        let pre = refund_pre_states();
        let post = execute_refund(&pre);

        // Maker received funds back
        assert_eq!(post[0].account().balance, 500 + AMOUNT);
        assert!(!post[0].requires_claim());

        // Escrow drained, state updated
        assert_eq!(post[1].account().balance, 0);
        assert!(!post[1].requires_claim());
        let escrow = HTLCEscrow::from_bytes(&post[1].account().data);
        assert_eq!(escrow.state, HTLCState::Refunded);
    }

    #[test]
    #[should_panic(expected = "only maker can refund")]
    fn test_refund_wrong_maker() {
        let mut pre = refund_pre_states();
        pre[0].account_id = wrong_id();
        execute_refund(&pre);
    }

    #[test]
    #[should_panic(expected = "maker must be authorized")]
    fn test_refund_not_authorized() {
        let mut pre = refund_pre_states();
        pre[0].is_authorized = false;
        execute_refund(&pre);
    }

    #[test]
    #[should_panic(expected = "escrow must be Locked")]
    fn test_refund_already_claimed() {
        let mut pre = refund_pre_states();
        pre[1].account.data = escrow_data_with_state(HTLCState::Claimed)
            .try_into()
            .expect("fits");
        execute_refund(&pre);
    }

    #[test]
    #[should_panic(expected = "escrow must be Locked")]
    fn test_refund_already_refunded() {
        let mut pre = refund_pre_states();
        pre[1].account.data = escrow_data_with_state(HTLCState::Refunded)
            .try_into()
            .expect("fits");
        execute_refund(&pre);
    }

    // ── Cross-chain compatibility tests ─────────────────────────────
    // These constants must match the Solidity test suite in
    // contracts/test/EthHTLC.t.sol (XCHAIN_PREIMAGE, XCHAIN_HASHLOCK).
    // If either side changes SHA-256 behavior, one of these tests breaks.

    const XCHAIN_PREIMAGE: &[u8; 32] = b"secret_preimage_for_testing_1234";
    const XCHAIN_HASHLOCK: [u8; 32] = [
        0x0e, 0xf6, 0x96, 0x11, 0xa9, 0x1e, 0x08, 0x05,
        0x07, 0x93, 0x87, 0xfe, 0xe0, 0xb8, 0x9f, 0xb7,
        0xd6, 0xfc, 0xd5, 0x05, 0x22, 0x0d, 0x40, 0x7b,
        0xac, 0xaa, 0x40, 0xce, 0x03, 0x17, 0x45, 0xdf,
    ];

    #[test]
    fn test_crosschain_sha256_compatibility() {
        // Verify that risc0's SHA-256 of our shared preimage matches
        // the hardcoded hashlock (same value asserted in the Solidity tests).
        let computed: [u8; 32] = Impl::hash_bytes(XCHAIN_PREIMAGE)
            .as_bytes()
            .try_into()
            .unwrap();
        assert_eq!(computed, XCHAIN_HASHLOCK);
    }

    #[test]
    fn test_crosschain_lock_then_claim_with_shared_preimage() {
        // Simulate the LEZ side of a cross-chain atomic swap.
        // Maker locks lambda using the shared hashlock.
        // Taker claims lambda using the shared preimage (learned from
        // the Maker's Ethereum claim which revealed it on-chain).
        let lock_pre = vec![
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: 0,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: true,
                account_id: maker_id(),
            },
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: AMOUNT,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: false,
                account_id: AccountId::new([0xEE; 32]),
            },
        ];

        let lock_post = execute_lock(&lock_pre, XCHAIN_HASHLOCK, taker_id(), AMOUNT);
        assert!(lock_post[1].requires_claim());

        // Now simulate claim: taker uses the preimage revealed on Ethereum.
        let claim_pre = vec![
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: 500,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: true,
                account_id: taker_id(),
            },
            AccountWithMetadata {
                account: lock_post[1].account().clone(),
                is_authorized: false,
                account_id: AccountId::new([0xEE; 32]),
            },
        ];

        let claim_post = execute_claim(&claim_pre, XCHAIN_PREIMAGE);

        assert_eq!(claim_post[0].account().balance, 500 + AMOUNT);
        let escrow = HTLCEscrow::from_bytes(&claim_post[1].account().data);
        assert_eq!(escrow.state, HTLCState::Claimed);
        assert_eq!(escrow.preimage, Some(XCHAIN_PREIMAGE.to_vec()));
    }

    #[test]
    fn test_crosschain_refund_after_timeout() {
        // Both parties refund: Maker refunds on LEZ, Taker refunds on Ethereum.
        // This test covers the LEZ side of the refund path.
        let lock_pre = vec![
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: 500,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: true,
                account_id: maker_id(),
            },
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: AMOUNT,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: false,
                account_id: AccountId::new([0xEE; 32]),
            },
        ];

        let lock_post = execute_lock(&lock_pre, XCHAIN_HASHLOCK, taker_id(), AMOUNT);

        // Maker refunds (CLI enforced timelock off-chain)
        let refund_pre = vec![
            AccountWithMetadata {
                account: Account {
                    program_owner: DEFAULT_PROGRAM_ID,
                    balance: 500,
                    data: Default::default(),
                    nonce: 0,
                },
                is_authorized: true,
                account_id: maker_id(),
            },
            AccountWithMetadata {
                account: lock_post[1].account().clone(),
                is_authorized: false,
                account_id: AccountId::new([0xEE; 32]),
            },
        ];

        let refund_post = execute_refund(&refund_pre);

        assert_eq!(refund_post[0].account().balance, 500 + AMOUNT);
        let escrow = HTLCEscrow::from_bytes(&refund_post[1].account().data);
        assert_eq!(escrow.state, HTLCState::Refunded);
    }
}
