//! The taker-LEZ-account binding, end to end over a real chain.
//!
//! This is the test the whole change hangs on: the LEZ account a taker
//! publishes in its OWN Ethereum lock must survive the round trip
//!
//!     EthClient::lock  →  Locked log  →  watcher decode  →  maker
//!     classification  →  the LEZ `Lock` instruction's `taker_id`
//!
//! unchanged, with NOTHING pre-agreed off-chain. If any link drops or rewrites
//! it, the maker locks LEZ to an account that cannot claim and the swap
//! strands.
//!
//! Anvil only — no LEZ sequencer, no scaffold wallet — so it can run in CI on
//! every PR. (`tests/e2e_swap.rs` covers the same path against a live localnet,
//! but it needs `make setup` and is `#[ignore]`d.)

use std::time::Duration;

use alloy::{
    node_bindings::Anvil,
    primitives::U256,
    providers::{Provider, ProviderBuilder, WsConnect},
    signers::local::PrivateKeySigner,
    sol,
};
use lee::AccountId;
use lez_htlc_program::HTLCInstruction;
use sha2::{Digest, Sha256};
use swap_orchestrator::{
    config::{LezAuth, SwapConfig},
    eth::client::EthClient,
    eth::watcher::{self, EthHtlcEvent},
    lez::client::build_lock_instruction,
    swap::maker::{
        Candidate, CandidateDisposition, MakerPolicy, RejectReason, classify_candidate,
        designated_lez_claimant,
    },
};
use tokio::sync::mpsc;

sol! {
    #[sol(rpc)]
    EthHTLC,
    "contracts/out/EthHTLC.sol/EthHTLC.json"
}

/// The taker's real LEZ account, known ONLY to the taker until it lands on
/// Ethereum. Distinct from the maker's, as any honest swap requires.
const TAKER_LEZ: [u8; 32] = [0x7Eu8; 32];
/// The maker's own LEZ account.
const MAKER_LEZ: [u8; 32] = [0xC0u8; 32];

const LEZ_AMOUNT: u128 = 1_000;
const ETH_AMOUNT: u128 = 1_000_000;

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A config good enough for `EthClient` (the LEZ half is never contacted here).
fn eth_only_config(
    ws: String,
    eth_private_key: String,
    contract: alloy::primitives::Address,
    recipient: alloy::primitives::Address,
    eth_timelock: u64,
) -> SwapConfig {
    SwapConfig {
        eth_rpc_url: ws,
        eth_private_key,
        eth_htlc_address: contract,
        lez_sequencer_url: "http://127.0.0.1:1".into(),
        lez_sequencer_url_explicit: true,
        lez_auth: LezAuth::RawKey(hex::encode([1u8; 32])),
        lez_htlc_program_id: [0u32; 8],
        lez_amount: LEZ_AMOUNT,
        eth_amount: ETH_AMOUNT,
        lez_timelock: now_unix() + 300,
        eth_timelock,
        eth_recipient_address: recipient,
        // THE point: the maker has no configured counterparty at all.
        lez_taker_account_id: None,
        poll_interval: Duration::from_millis(200),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn taker_lez_account_travels_from_eth_lock_to_lez_lock_instruction() {
    let anvil = Anvil::new().block_time(1).try_spawn().unwrap();
    let maker_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let maker_eth_addr = maker_signer.address();

    let deployer = ProviderBuilder::new()
        .wallet(maker_signer)
        .connect_ws(WsConnect::new(anvil.ws_endpoint()))
        .await
        .unwrap()
        .erased();
    let contract = EthHTLC::deploy(&deployer, U256::from(60u64)).await.unwrap();
    let contract_addr = *contract.address();

    let preimage = [0xABu8; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();
    let eth_timelock = now_unix() + 3600;

    // ── the maker: watching, with NO designated counterparty configured ──
    let maker_config = eth_only_config(
        anvil.ws_endpoint(),
        hex::encode(anvil.keys()[0].to_bytes()),
        contract_addr,
        maker_eth_addr,
        eth_timelock,
    );
    assert!(
        maker_config.lez_taker_account_id.is_none(),
        "the maker must learn the taker's LEZ account from the chain, not from config"
    );
    let maker_eth = EthClient::new(&maker_config).await.unwrap();
    let (tx, mut rx) = mpsc::channel::<EthHtlcEvent>(16);
    let watcher_client = EthClient::new(&maker_config).await.unwrap();
    let watcher_handle = tokio::spawn(async move {
        let _ = watcher::watch_events(&watcher_client, tx).await;
    });

    // ── the taker: locks ETH, publishing its own LEZ account ──
    let taker_config = eth_only_config(
        anvil.ws_endpoint(),
        hex::encode(anvil.keys()[1].to_bytes()),
        contract_addr,
        maker_eth_addr,
        eth_timelock,
    );
    let taker_eth = EthClient::new(&taker_config).await.unwrap();
    let swap_id = taker_eth
        .lock(
            hashlock,
            eth_timelock,
            maker_eth_addr,
            TAKER_LEZ,
            U256::from(ETH_AMOUNT),
        )
        .await
        .expect("taker locks ETH");

    // ── link 1: the watcher decodes the account off the log ──
    let event = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match rx.recv().await {
                Some(EthHtlcEvent::Locked {
                    swap_id: id,
                    taker_lez_account,
                    hashlock: hl,
                    timelock,
                    ..
                }) if id == swap_id => return (taker_lez_account, hl, timelock),
                Some(_) => continue,
                None => panic!("watcher channel closed"),
            }
        }
    })
    .await
    .expect("watcher must deliver the Locked event");
    watcher_handle.abort();

    let (event_taker_lez, event_hashlock, event_timelock) = event;
    assert_eq!(
        event_taker_lez.0, TAKER_LEZ,
        "the watcher must surface the taker's LEZ account verbatim"
    );
    assert_eq!(event_hashlock.0, hashlock);

    // ── link 2: the maker accepts it (no config, stranger taker) ──
    let disposition = classify_candidate(
        &Candidate {
            hashlock: &event_hashlock.0,
            taker_lez_account: &event_taker_lez.0,
            eth_timelock_secs: event_timelock.saturating_to::<u64>(),
        },
        &MakerPolicy {
            maker_lez_account: &MAKER_LEZ,
            designated_taker: maker_config.lez_taker_account_id.map(|_| &MAKER_LEZ),
            lez_expiry_secs: now_unix() + 300,
            margin: Some(300),
        },
        &Default::default(),
        false,
    );
    assert_eq!(
        disposition,
        CandidateDisposition::Accept,
        "a stranger's well-formed lock must be served"
    );

    // ── link 3: the account reaches the LEZ Lock instruction ──
    let claimant = designated_lez_claimant(&event_taker_lez.0, &MAKER_LEZ)
        .expect("a stranger's account is usable");
    let instruction = build_lock_instruction(event_hashlock.0, claimant, LEZ_AMOUNT, 300);
    match instruction {
        HTLCInstruction::Lock { taker_id, .. } => assert_eq!(
            taker_id,
            AccountId::new(TAKER_LEZ),
            "the LEZ escrow must name the taker's OWN account — the guest gates Claim on it"
        ),
        other => panic!("expected a Lock instruction, got {other:?}"),
    }

    // ── and the hostile case, over the same real chain ──
    // A taker who names the MAKER'S account (the guest asserts maker != taker,
    // so the maker's Lock would fail) is rejected at classification, i.e.
    // before any journal write or LEZ instruction exists.
    let hostile_preimage = [0xCDu8; 32];
    let hostile_hashlock: [u8; 32] = Sha256::digest(hostile_preimage).into();
    taker_eth
        .lock(
            hostile_hashlock,
            eth_timelock,
            maker_eth_addr,
            MAKER_LEZ,
            U256::from(ETH_AMOUNT),
        )
        .await
        .expect("the contract itself accepts it — only the maker can judge it");

    assert_eq!(
        classify_candidate(
            &Candidate {
                hashlock: &hostile_hashlock,
                taker_lez_account: &MAKER_LEZ,
                eth_timelock_secs: eth_timelock,
            },
            &MakerPolicy {
                maker_lez_account: &MAKER_LEZ,
                designated_taker: None,
                lez_expiry_secs: now_unix() + 300,
                margin: Some(300),
            },
            &Default::default(),
            false,
        ),
        CandidateDisposition::Reject(RejectReason::UnusableTakerLezAccount)
    );
    assert!(designated_lez_claimant(&MAKER_LEZ, &MAKER_LEZ).is_none());

    // Sanity: the honest lock really is OPEN on-chain with the account stored.
    let htlc = maker_eth.get_htlc(swap_id).await.unwrap();
    assert_eq!(htlc.takerLezAccount.0, TAKER_LEZ);
}
