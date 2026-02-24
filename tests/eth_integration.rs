use alloy::{
    node_bindings::Anvil,
    primitives::U256,
    providers::{Provider, ProviderBuilder, WsConnect},
    signers::local::PrivateKeySigner,
    sol,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

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

    (maker_provider, taker_provider, contract_addr, maker_addr, taker_addr, anvil)
}

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
        .lock(hashlock.into(), timelock, taker_addr)
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
        .lock(hashlock.into(), timelock, taker_addr)
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
        .lock(hashlock.into(), timelock, taker_addr)
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
        .raw_request("evm_increaseTime".into(), &[U256::from(300)])
        .await
        .unwrap();
    let _: serde_json::Value = maker
        .raw_request("evm_mine".into(), &())
        .await
        .unwrap();

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
        .lock(hashlock.into(), timelock, taker_addr)
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

    assert_eq!(event.0.hashlock, alloy::primitives::FixedBytes::from(hashlock));
    assert_eq!(event.0.recipient, taker_addr);
}
