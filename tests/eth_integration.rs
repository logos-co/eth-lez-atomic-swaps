use alloy::{
    node_bindings::Anvil,
    primitives::{FixedBytes, U256},
    providers::{Provider, ProviderBuilder, WsConnect},
    signers::local::PrivateKeySigner,
    sol,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use swap_orchestrator::{
    config::{LezAuth, SwapConfig},
    eth::client::EthClient,
    eth::watcher::{self, EthHtlcEvent},
};

// Reads ABI + bytecode from the Foundry build artifact.
// Run `cd contracts && forge build`.
sol! {
    #[sol(rpc)]
    EthHTLC,
    "contracts/out/EthHTLC.sol/EthHTLC.json"
}

async fn setup() -> (
    alloy::providers::DynProvider,
    alloy::providers::DynProvider,
    alloy::primitives::Address,
    alloy::primitives::Address,
    alloy::primitives::Address,
    alloy::node_bindings::AnvilInstance,
) {
    let anvil = Anvil::new().block_time(1).try_spawn().unwrap();

    let maker_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let taker_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
    let maker_addr = maker_signer.address();
    let taker_addr = taker_signer.address();

    let maker_provider = ProviderBuilder::new()
        .wallet(maker_signer)
        .connect_ws(WsConnect::new(anvil.ws_endpoint()))
        .await
        .unwrap()
        .erased();

    let contract = EthHTLC::deploy(&maker_provider, U256::from(60))
        .await
        .unwrap();
    let contract_addr = *contract.address();

    let taker_provider = ProviderBuilder::new()
        .wallet(taker_signer)
        .connect_ws(WsConnect::new(anvil.ws_endpoint()))
        .await
        .unwrap()
        .erased();

    (
        maker_provider,
        taker_provider,
        contract_addr,
        maker_addr,
        taker_addr,
        anvil,
    )
}

/// A stand-in taker LEZ AccountId. `lock()` rejects `bytes32(0)`, and the value
/// is part of the swapId preimage, so it must be a real 32-byte account.
const TAKER_LEZ_ACCOUNT: [u8; 32] = [0x7Eu8; 32];

fn make_preimage_and_hashlock() -> ([u8; 32], [u8; 32]) {
    let preimage = [0xABu8; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();
    (preimage, hashlock)
}

async fn future_timelock(provider: &alloy::providers::DynProvider) -> U256 {
    let block = provider.get_block_number().await.unwrap();
    let ts = provider
        .get_block_by_number(block.into())
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;
    U256::from(ts + 3600)
}

// ---------- Tests ----------

#[tokio::test]
async fn test_lock_and_read() {
    let (maker, _taker, contract_addr, _maker_addr, taker_addr, _anvil) = setup().await;

    let (_, hashlock) = make_preimage_and_hashlock();
    let timelock = future_timelock(&maker).await;
    let amount = U256::from(1_000_000);

    let contract = EthHTLC::new(contract_addr, maker.clone());
    let receipt = contract
        .lock(hashlock.into(), timelock, taker_addr, TAKER_LEZ_ACCOUNT.into())
        .value(amount)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    assert!(receipt.status());

    let swap_id = receipt.inner.logs()[0].topics()[1];
    let htlc = EthHTLC::new(contract_addr, maker.clone())
        .getHTLC(swap_id)
        .call()
        .await
        .unwrap();

    assert_eq!(htlc.state, 1); // OPEN
    assert_eq!(htlc.amount, amount);
}

#[tokio::test]
async fn test_lock_and_claim() {
    let (maker, taker, contract_addr, _maker_addr, taker_addr, _anvil) = setup().await;

    let (preimage, hashlock) = make_preimage_and_hashlock();
    let timelock = future_timelock(&maker).await;
    let amount = U256::from(1_000_000);

    let maker_contract = EthHTLC::new(contract_addr, maker.clone());
    let receipt = maker_contract
        .lock(hashlock.into(), timelock, taker_addr, TAKER_LEZ_ACCOUNT.into())
        .value(amount)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let swap_id = receipt.inner.logs()[0].topics()[1];

    // Claim as taker.
    let taker_contract = EthHTLC::new(contract_addr, taker.clone());
    let claim_receipt = taker_contract
        .claim(swap_id, preimage.into())
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    assert!(claim_receipt.status());

    let htlc = EthHTLC::new(contract_addr, maker.clone())
        .getHTLC(swap_id)
        .call()
        .await
        .unwrap();
    assert_eq!(htlc.state, 2); // CLAIMED
}

#[tokio::test]
async fn test_lock_and_refund() {
    let (maker, _taker, contract_addr, _maker_addr, taker_addr, _anvil) = setup().await;

    let (_, hashlock) = make_preimage_and_hashlock();
    let block = maker.get_block_number().await.unwrap();
    let ts = maker
        .get_block_by_number(block.into())
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;
    let timelock = U256::from(ts + 120);
    let amount = U256::from(1_000_000);

    let contract = EthHTLC::new(contract_addr, maker.clone());
    let receipt = contract
        .lock(hashlock.into(), timelock, taker_addr, TAKER_LEZ_ACCOUNT.into())
        .value(amount)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let swap_id = receipt.inner.logs()[0].topics()[1];

    // Fast-forward time past the timelock.
    let _: serde_json::Value = maker
        .raw_request("evm_increaseTime".into(), [U256::from(300)])
        .await
        .unwrap();
    let _: serde_json::Value = maker.raw_request("evm_mine".into(), ()).await.unwrap();

    let refund_receipt = contract
        .refund(swap_id)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    assert!(refund_receipt.status());

    let htlc = EthHTLC::new(contract_addr, maker.clone())
        .getHTLC(swap_id)
        .call()
        .await
        .unwrap();
    assert_eq!(htlc.state, 3); // REFUNDED
}

#[tokio::test]
async fn test_watcher_receives_locked_event() {
    let (maker, _taker, contract_addr, _maker_addr, taker_addr, _anvil) = setup().await;

    let (_, hashlock) = make_preimage_and_hashlock();
    let timelock = future_timelock(&maker).await;

    // Subscribe before sending the tx.
    let watcher_contract = EthHTLC::new(contract_addr, maker.clone());
    let locked_watch = watcher_contract.Locked_filter().watch().await.unwrap();

    // Lock ETH.
    watcher_contract
        .lock(hashlock.into(), timelock, taker_addr, TAKER_LEZ_ACCOUNT.into())
        .value(U256::from(1_000_000))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Receive the event from the stream.
    let mut stream = locked_watch.into_stream();
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .expect("timed out waiting for Locked event")
        .expect("stream ended")
        .expect("decode error");

    assert_eq!(
        event.0.hashlock,
        alloy::primitives::FixedBytes::from(hashlock)
    );
    assert_eq!(event.0.recipient, taker_addr);
    // The taker's LEZ account must survive the round trip through the log — it
    // is the value the maker binds its LEZ escrow to.
    assert_eq!(
        event.0.takerLezAccount,
        alloy::primitives::FixedBytes::from(TAKER_LEZ_ACCOUNT)
    );
}

// Two locks that differ ONLY in takerLezAccount must get distinct swap ids and
// coexist — the property the maker's matching relies on, and the reason its
// dedupe key had to move to the hashlock (one sender can mint unlimited swap
// ids for a single hashlock this way).
#[tokio::test]
async fn test_taker_lez_account_varies_swap_id_for_one_hashlock() {
    let (maker, _taker, contract_addr, _maker_addr, taker_addr, _anvil) = setup().await;

    let (_, hashlock) = make_preimage_and_hashlock();
    let timelock = future_timelock(&maker).await;
    let contract = EthHTLC::new(contract_addr, maker.clone());

    let mut swap_ids = Vec::new();
    for account in [[0x01u8; 32], [0x02u8; 32]] {
        let receipt = contract
            .lock(hashlock.into(), timelock, taker_addr, account.into())
            .value(U256::from(1_000_000))
            .send()
            .await
            .unwrap()
            .get_receipt()
            .await
            .unwrap();
        assert!(receipt.status());
        swap_ids.push(receipt.inner.logs()[0].topics()[1]);
    }

    assert_ne!(
        swap_ids[0], swap_ids[1],
        "takerLezAccount is in the swapId preimage, so the two locks must not collide"
    );
    // Both are simultaneously OPEN: the ETH side allows many escrows per
    // hashlock while LEZ allows exactly one, which is why maker-side dedupe
    // must key on the hashlock rather than the swap id.
    for id in swap_ids {
        let htlc = contract.getHTLC(id).call().await.unwrap();
        assert_eq!(htlc.state, 1, "both locks are OPEN at once");
    }
}

// The startup handshake, against real deployments. A current contract passes;
// the check is what stops a version-skewed build from silently never seeing
// lock events.
#[tokio::test]
async fn test_interface_version_handshake() {
    let (maker, _taker, contract_addr, _maker_addr, _taker_addr, _anvil) = setup().await;

    let reported = EthHTLC::new(contract_addr, maker.clone())
        .INTERFACE_VERSION()
        .call()
        .await
        .expect("deployed EthHTLC must expose INTERFACE_VERSION");
    assert_eq!(
        reported.to::<u64>(),
        swap_orchestrator::eth::client::EXPECTED_INTERFACE_VERSION,
        "contract and app build must agree on the interface version"
    );

    // A contract WITHOUT the getter (here: an address with no code at all) must
    // be rejected, not silently accepted.
    let empty = alloy::primitives::Address::from([0x9Au8; 20]);
    assert!(
        swap_orchestrator::eth::client::verify_interface_version(&maker, empty)
            .await
            .is_err(),
        "an unversioned/absent contract must fail the handshake"
    );
    assert!(
        swap_orchestrator::eth::client::verify_interface_version(&maker, contract_addr)
            .await
            .is_ok()
    );
}

/// Runtime evidence for the receipt-linkability fix: drives the actual
/// production `EthClient`/`watcher` code (not the raw `sol!` contract used by
/// the tests above) and asserts that `EthClient::lock` returns a real,
/// non-zero tx hash alongside the swap id, that `EthClient::chain_id` reports
/// Anvil's chain id (31337), and that the watcher's `EthHtlcEvent::Locked`
/// carries that SAME tx hash — the maker's only path to the taker's lock tx.
///
/// This substitutes for a full `make demo` run (which requires bootstrapping
/// a logos-scaffold LEZ localnet via nix — no `.scaffold/` state exists in
/// this environment and standing one up from scratch is a multi-minute, not
/// guaranteed to succeed operation unrelated to the ETH-side change under
/// test). It exercises the same `EthClient::lock` -> `EthLockReceipt` and
/// `watcher::watch_events` -> `EthHtlcEvent::Locked` code paths that
/// `src/swap/taker.rs` and `src/swap/maker.rs` call in the real swap flow.
#[tokio::test]
async fn test_eth_client_lock_and_watcher_report_tx_hash_and_chain_id() {
    let (maker, _taker, contract_addr, maker_addr, taker_addr, anvil) = setup().await;

    let (_, hashlock) = make_preimage_and_hashlock();
    let block = maker.get_block_number().await.unwrap();
    let ts = maker
        .get_block_by_number(block.into())
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;
    let timelock: u64 = ts + 3600;
    let amount: u128 = 1_000_000;

    let config = SwapConfig {
        eth_rpc_url: anvil.ws_endpoint(),
        eth_private_key: hex::encode(anvil.keys()[0].to_bytes()),
        eth_htlc_address: contract_addr,
        lez_sequencer_url: "http://127.0.0.1:0".to_string(),
        lez_sequencer_url_explicit: false,
        lez_auth: LezAuth::RawKey("bb".repeat(32)),
        lez_htlc_program_id: [0u32; 8],
        lez_amount: 1,
        eth_amount: amount,
        lez_timelock: timelock,
        eth_timelock: timelock,
        eth_recipient_address: taker_addr,
        // Unset: this build's maker serves whoever locks ETH, binding each swap
        // to the LEZ account carried on the taker's own lock.
        lez_taker_account_id: None,
        poll_interval: std::time::Duration::from_millis(200),
    };

    // Drives src/eth/client.rs's actual EthClient::new, which now fetches
    // and caches the chain id via provider.get_chain_id().
    let eth_client = EthClient::new(&config).await.unwrap();
    println!("[evidence] chain_id: {}", eth_client.chain_id());
    assert_eq!(
        eth_client.chain_id(),
        31337,
        "Anvil's default chain id must be surfaced by EthClient::chain_id()"
    );

    // Drives src/eth/client.rs's actual EthClient::lock, which now returns
    // an EthLockReceipt{swap_id, tx_hash} instead of a bare swap id.
    let lock_receipt = eth_client
        .lock(
            hashlock,
            timelock,
            taker_addr,
            TAKER_LEZ_ACCOUNT,
            U256::from(amount),
        )
        .await
        .unwrap();
    println!(
        "[evidence] EthLockReceipt {{ swap_id: {}, tx_hash: {} }}",
        lock_receipt.swap_id, lock_receipt.tx_hash
    );

    assert_ne!(
        lock_receipt.swap_id,
        FixedBytes::ZERO,
        "swap_id must be a real, non-zero id decoded from the Locked event"
    );
    assert_ne!(
        lock_receipt.tx_hash,
        FixedBytes::ZERO,
        "tx_hash must be a real, non-zero transaction hash — this is what a \
         block-explorer link needs"
    );

    // Confirm it is a REAL, minable transaction hash (not a placeholder) by
    // reading it straight back from the chain.
    let tx = eth_client
        .provider()
        .get_transaction_by_hash(lock_receipt.tx_hash)
        .await
        .unwrap()
        .expect("tx_hash must resolve to a real on-chain transaction");
    assert_eq!(tx.inner.signer(), maker_addr);

    // Drives src/eth/watcher.rs's actual watch_events, which now binds the
    // `Log` (previously discarded via `for (event, _) in events`) and carries
    // its transaction_hash on EthHtlcEvent::Locked. The 256-block replay on
    // startup picks up the lock tx that already landed above.
    let (tx_chan, mut rx) = tokio::sync::mpsc::channel::<EthHtlcEvent>(16);
    let watcher_handle = tokio::spawn({
        let config = config.clone();
        async move {
            let watcher_client = EthClient::new(&config).await.unwrap();
            let _ = watcher::watch_events(&watcher_client, tx_chan).await;
        }
    });

    let event = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Some(EthHtlcEvent::Locked {
                    swap_id, tx_hash, ..
                }) if swap_id == lock_receipt.swap_id => return tx_hash,
                Some(_) => continue,
                None => panic!("watcher channel closed before delivering the Locked event"),
            }
        }
    })
    .await
    .expect("timed out waiting for the watcher to replay the Locked event");

    watcher_handle.abort();
    println!("[evidence] EthHtlcEvent::Locked.tx_hash: {event}");

    assert_eq!(
        event, lock_receipt.tx_hash,
        "the watcher's EthHtlcEvent::Locked.tx_hash must match the actual lock \
         tx hash — this is the maker's only path to the taker's lock tx"
    );
}
