//! The on-chain swap count, end to end against a real chain.
//!
//! `src/eth/report.rs`'s unit tests prove the counting rules in isolation; this
//! proves the half they cannot reach — that real `Locked`/`Claimed`/`Refunded`
//! logs emitted by the real contract decode into those rules correctly, over
//! real block ranges. A silent decode regression here would look exactly like a
//! quiet trial: zero attempts, no error.
//!
//! Anvil only — no LEZ sequencer, no scaffold wallet — so it gates every PR
//! alongside `tests/eth_integration.rs` and `tests/taker_binding.rs`.

use alloy::{
    node_bindings::Anvil,
    primitives::{Address, U256},
    providers::{DynProvider, Provider, ProviderBuilder, WsConnect},
    signers::local::PrivateKeySigner,
    sol,
};
use sha2::{Digest, Sha256};
use swap_orchestrator::error::SwapError;
use swap_orchestrator::eth::report::{ReportWindow, collect_tally};

// Reads ABI + bytecode from the Foundry build artifact.
// Run `cd contracts && forge build`.
sol! {
    #[sol(rpc)]
    EthHTLC,
    "contracts/out/EthHTLC.sol/EthHTLC.json"
}

/// A stand-in taker LEZ AccountId. `lock()` rejects `bytes32(0)`, and the value
/// is part of the swapId preimage, so it must be a real 32-byte account.
const TAKER_LEZ_ACCOUNT: [u8; 32] = [0x7Eu8; 32];

/// Small enough that a lock is immediately refundable after one time warp.
const MIN_TIMELOCK_DELTA: u64 = 60;

const CHUNK: u64 = 1_000;

struct Venue {
    anvil: alloy::node_bindings::AnvilInstance,
    address: Address,
    /// Indexed by anvil key: 0 = deployer, 1..=2 = takers, 3..=4 = makers.
    providers: Vec<DynProvider>,
}

impl Venue {
    fn taker(&self, i: usize) -> &DynProvider {
        &self.providers[1 + i]
    }
    fn maker(&self, i: usize) -> &DynProvider {
        &self.providers[3 + i]
    }
    fn maker_address(&self, i: usize) -> Address {
        let signer: PrivateKeySigner = self.anvil.keys()[3 + i].clone().into();
        signer.address()
    }
    fn taker_address(&self, i: usize) -> Address {
        let signer: PrivateKeySigner = self.anvil.keys()[1 + i].clone().into();
        signer.address()
    }
}

async fn setup() -> Venue {
    let anvil = Anvil::new().block_time(1).try_spawn().unwrap();

    let mut providers = Vec::new();
    for key in anvil.keys().iter().take(5) {
        let signer: PrivateKeySigner = key.clone().into();
        providers.push(
            ProviderBuilder::new()
                .wallet(signer)
                .connect_ws(WsConnect::new(anvil.ws_endpoint()))
                .await
                .unwrap()
                .erased(),
        );
    }

    let contract = EthHTLC::deploy(&providers[0], U256::from(MIN_TIMELOCK_DELTA))
        .await
        .unwrap();
    let address = *contract.address();

    Venue {
        anvil,
        address,
        providers,
    }
}

async fn now_ts(provider: &DynProvider) -> u64 {
    let block = provider.get_block_number().await.unwrap();
    provider
        .get_block_by_number(block.into())
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp
}

/// One lock, from `taker` to `recipient`. `nonce` only varies the preimage so
/// each lock gets a distinct hashlock (and therefore a distinct swapId).
/// Returns `(swap_id, preimage, block_number)`.
async fn lock(
    venue: &Venue,
    taker: &DynProvider,
    recipient: Address,
    nonce: u8,
    timelock_secs: u64,
) -> (alloy::primitives::FixedBytes<32>, [u8; 32], u64) {
    let preimage = [nonce; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();
    let timelock = U256::from(now_ts(taker).await + timelock_secs);

    let receipt = EthHTLC::new(venue.address, taker.clone())
        .lock(
            hashlock.into(),
            timelock,
            recipient,
            TAKER_LEZ_ACCOUNT.into(),
        )
        .value(U256::from(1_000_000u64))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status(), "lock reverted");

    let swap_id = receipt.inner.logs()[0].topics()[1];
    (swap_id, preimage, receipt.block_number.unwrap())
}

async fn claim(
    venue: &Venue,
    maker: &DynProvider,
    swap_id: alloy::primitives::FixedBytes<32>,
    preimage: [u8; 32],
) -> u64 {
    let receipt = EthHTLC::new(venue.address, maker.clone())
        .claim(swap_id, preimage.into())
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status(), "claim reverted");
    receipt.block_number.unwrap()
}

async fn warp(provider: &DynProvider, secs: u64) {
    let _: serde_json::Value = provider
        .raw_request("evm_increaseTime".into(), [U256::from(secs)])
        .await
        .unwrap();
    let _: serde_json::Value = provider.raw_request("evm_mine".into(), ()).await.unwrap();
}

async fn refund(
    venue: &Venue,
    taker: &DynProvider,
    swap_id: alloy::primitives::FixedBytes<32>,
) -> u64 {
    let receipt = EthHTLC::new(venue.address, taker.clone())
        .refund(swap_id)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status(), "refund reverted");
    receipt.block_number.unwrap()
}

async fn head(provider: &DynProvider) -> u64 {
    provider.get_block_number().await.unwrap()
}

/// A miniature public trial: two takers, two makers, one of each outcome.
///
/// This is the whole acceptance criterion in one place — attempted / completed
/// / refunded / still-open, distinct takers, and the per-maker split, all
/// derived from nothing but the contract's own logs.
#[tokio::test]
async fn counts_a_mixed_trial_from_logs_alone() {
    let venue = setup().await;
    let (m1, m2) = (venue.maker_address(0), venue.maker_address(1));
    let start = head(venue.taker(0)).await;

    // taker 0 → maker 0, completed.
    let (s1, p1, _) = lock(&venue, venue.taker(0), m1, 0x01, 3_600).await;
    claim(&venue, venue.maker(0), s1, p1).await;

    // taker 0 → maker 1, refunded (the taker nobody served — the case the ops
    // ledger is structurally blind to).
    let (s2, _, _) = lock(&venue, venue.taker(0), m2, 0x02, 120).await;

    // taker 1 → maker 0, still open.
    let (_s3, _, _) = lock(&venue, venue.taker(1), m1, 0x03, 3_600).await;

    // taker 1 → maker 0, completed.
    let (s4, p4, _) = lock(&venue, venue.taker(1), m1, 0x04, 3_600).await;
    claim(&venue, venue.maker(0), s4, p4).await;

    warp(venue.taker(0), 300).await;
    refund(&venue, venue.taker(0), s2).await;

    let to = head(venue.taker(0)).await;
    let window = ReportWindow {
        locks_from: start,
        locks_to: to,
        outcomes_to: to,
    };
    let counts = collect_tally(venue.taker(0), venue.address, &window, CHUNK, false)
        .await
        .unwrap()
        .tally
        .counts();

    assert_eq!(counts.attempted, 4);
    assert_eq!(counts.completed, 2);
    assert_eq!(counts.refunded, 1);
    assert_eq!(counts.still_open, 1);
    assert_eq!(
        counts.completed + counts.refunded + counts.still_open,
        counts.attempted
    );
    assert_eq!(
        counts.distinct_taker_wallets, 2,
        "two wallets locked, across four swaps"
    );

    // Per-maker split, keyed on Locked.recipient.
    assert_eq!(counts.by_maker.len(), 2);
    let of = |a: Address| {
        counts
            .by_maker
            .iter()
            .find(|m| m.recipient == a.to_string())
            .unwrap_or_else(|| panic!("no row for {a}"))
    };
    let r1 = of(m1);
    assert_eq!(
        (r1.attempted, r1.completed, r1.refunded, r1.still_open),
        (3, 2, 0, 1)
    );
    let r2 = of(m2);
    assert_eq!(
        (r2.attempted, r2.completed, r2.refunded, r2.still_open),
        (1, 0, 1, 0)
    );

    // No taker address may leak into the report, even though every one of them
    // is sitting right there in the logs the tally just read.
    let json = serde_json::to_string(&counts).unwrap();
    for i in 0..2 {
        assert!(
            !json.contains(&venue.taker_address(i).to_string()),
            "a taker address reached the report"
        );
    }
}

/// A trial period measured after the fact: only locks inside the block window
/// are attempts, and locks on either side of it are invisible.
#[tokio::test]
async fn a_block_window_bounds_the_attempt_count() {
    let venue = setup().await;
    let m1 = venue.maker_address(0);

    // Before the window.
    lock(&venue, venue.taker(0), m1, 0x11, 3_600).await;
    warp(venue.taker(0), 1).await;
    let window_from = head(venue.taker(0)).await + 1;

    // Inside it.
    let (s_in, p_in, _) = lock(&venue, venue.taker(0), m1, 0x12, 3_600).await;
    claim(&venue, venue.maker(0), s_in, p_in).await;
    let window_to = head(venue.taker(0)).await;

    // After it.
    lock(&venue, venue.taker(1), m1, 0x13, 3_600).await;

    let counts = collect_tally(
        venue.taker(0),
        venue.address,
        &ReportWindow {
            locks_from: window_from,
            locks_to: window_to,
            outcomes_to: head(venue.taker(0)).await,
        },
        CHUNK,
        false,
    )
    .await
    .unwrap()
    .tally
    .counts();

    assert_eq!(counts.attempted, 1, "only the in-window lock is an attempt");
    assert_eq!(counts.completed, 1);
    assert_eq!(counts.distinct_taker_wallets, 1);
}

/// The still-open derivation's sharp edge: a swap locked in the last block of
/// the attempt window but claimed *after* it is completed, not open. Following
/// outcomes past `locks_to` is what makes a bounded trial window honest.
#[tokio::test]
async fn outcomes_are_followed_past_the_end_of_the_attempt_window() {
    let venue = setup().await;
    let m1 = venue.maker_address(0);
    let start = head(venue.taker(0)).await;

    let (swap_id, preimage, lock_block) = lock(&venue, venue.taker(0), m1, 0x21, 3_600).await;
    let claim_block = claim(&venue, venue.maker(0), swap_id, preimage).await;
    assert!(
        claim_block > lock_block,
        "the claim must land in a later block for this test to mean anything"
    );

    // Attempt window ends at the lock; outcomes followed to the head.
    let followed = collect_tally(
        venue.taker(0),
        venue.address,
        &ReportWindow {
            locks_from: start,
            locks_to: lock_block,
            outcomes_to: head(venue.taker(0)).await,
        },
        CHUNK,
        false,
    )
    .await
    .unwrap()
    .tally
    .counts();
    assert_eq!(followed.attempted, 1);
    assert_eq!(followed.completed, 1);
    assert_eq!(followed.still_open, 0);

    // The same window with outcomes cut off at the lock: the swap was genuinely
    // still open as of that block, and the report says so rather than guessing.
    let truncated = collect_tally(
        venue.taker(0),
        venue.address,
        &ReportWindow {
            locks_from: start,
            locks_to: lock_block,
            outcomes_to: lock_block,
        },
        CHUNK,
        false,
    )
    .await
    .unwrap()
    .tally
    .counts();
    assert_eq!(truncated.attempted, 1);
    assert_eq!(truncated.completed, 0);
    assert_eq!(truncated.still_open, 1);
}

/// A window with no locks reports zeroes, not an error — and a chunk size far
/// smaller than the range still covers every block exactly once.
#[tokio::test]
async fn empty_window_is_zero_and_small_chunks_still_cover_the_range() {
    let venue = setup().await;
    let m1 = venue.maker_address(0);
    let start = head(venue.taker(0)).await;

    lock(&venue, venue.taker(0), m1, 0x31, 3_600).await;
    let to = head(venue.taker(0)).await;

    // One block per eth_getLogs call: the chunking must not drop or double-count
    // the block the lock landed in.
    let counts = collect_tally(
        venue.taker(0),
        venue.address,
        &ReportWindow {
            locks_from: start,
            locks_to: to,
            outcomes_to: to,
        },
        1,
        false,
    )
    .await
    .unwrap()
    .tally
    .counts();
    assert_eq!(counts.attempted, 1);

    // An inverted/empty window is a no-op, not a query.
    let empty = collect_tally(
        venue.taker(0),
        venue.address,
        &ReportWindow {
            locks_from: to + 1,
            locks_to: to,
            outcomes_to: to,
        },
        CHUNK,
        false,
    )
    .await
    .unwrap()
    .tally
    .counts();
    assert_eq!(empty.attempted, 0);
    assert!(empty.by_maker.is_empty());
}

/// A scan whose anchor probes ALL came back empty proves nothing — that is what
/// a backend with no log index for the era looks like, and what a chain with no
/// logs looks like, and the responses do not tell them apart. Refusing is the
/// default; the operator has to assert the quiet chain.
///
/// This anvil has no logs at all (deploying EthHTLC emits none), which is
/// exactly the state a pruned backend fakes.
#[tokio::test]
async fn a_scan_without_an_anchor_is_refused_until_the_operator_opts_in() {
    let venue = setup().await;
    let to = head(venue.taker(0)).await;
    // Two blocks: enough to scan, few enough that the probe walk is quick.
    let window = ReportWindow {
        locks_from: to.saturating_sub(1),
        locks_to: to,
        outcomes_to: to,
    };

    let refused = collect_tally(venue.taker(0), venue.address, &window, CHUNK, false)
        .await
        .expect_err("an unprovable scan must not produce a headline number");
    let SwapError::EthLogsUncorroborated(msg) = refused else {
        panic!("expected an uncorroborated-logs refusal, got: {refused:?}");
    };
    assert!(
        msg.contains("--allow-uncorroborated"),
        "the refusal must name the opt-in: {msg}"
    );

    // The same scan, with the opt-in: it proceeds, and says it is unproven.
    let scan = collect_tally(venue.taker(0), venue.address, &window, CHUNK, true)
        .await
        .expect("--allow-uncorroborated downgrades the refusal");
    assert!(!scan.corroborated, "an unanchored count is never corroborated");
    assert_eq!(scan.tally.counts().attempted, 0);
}

/// The other half of the same decision: once the probes DO anchor, a window in
/// which the scanned contract has no logs is a *proven* zero and is reported,
/// not refused. This is the case the refusal must not swallow.
#[tokio::test]
async fn a_proven_zero_is_reported_rather_than_refused() {
    let venue = setup().await;
    let m1 = venue.maker_address(0);
    let start = head(venue.taker(0)).await;

    lock(&venue, venue.taker(0), m1, 0x41, 3_600).await;
    let to = head(venue.taker(0)).await;
    let window = ReportWindow {
        locks_from: start,
        locks_to: to,
        outcomes_to: to,
    };

    // These blocks demonstrably have logs, so the probes anchor — but they hold
    // none for THIS address, so its answer is genuinely empty.
    let quiet_venue = Address::from([0x5Au8; 20]);
    let scan = collect_tally(venue.taker(0), quiet_venue, &window, CHUNK, false)
        .await
        .expect("an anchored scan reports its zero instead of refusing");
    assert!(
        scan.corroborated,
        "the probes anchored, so the answer carries its proof"
    );
    assert_eq!(scan.tally.counts().attempted, 0);

    // And the same anchored window over the real venue is a proven one.
    let real = collect_tally(venue.taker(0), venue.address, &window, CHUNK, false)
        .await
        .unwrap();
    assert!(real.corroborated);
    assert_eq!(real.tally.counts().attempted, 1);
}
