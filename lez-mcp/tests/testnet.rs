//! Integration smoke tests against PUBLIC infrastructure (LEZ testnet +
//! Sepolia). Network-dependent, so `#[ignore]` by default; run explicitly:
//!
//! ```sh
//! cargo test -p lez-mcp --test testnet -- --ignored --nocapture
//! ```

use lez_mcp::{
    config::{DEFAULT_ETH_HTLC_ADDRESS, DEFAULT_ETH_HTLC_FROM_BLOCK, DEFAULT_ETH_RPC_URL,
             DEFAULT_SEQUENCER_URL},
    eth::{EthReader, parse_bytes32, state_str},
    fingerprint::run_fingerprint,
};
use sequencer_service_rpc::{RpcClient as _, SequencerClientBuilder};
use swap_orchestrator::config::parse_base58_account_id;

/// Maker account from the first public swap (docs/testnet.md) — known to
/// exist on the public testnet.
const KNOWN_ACCOUNT: &str = "AU4z2Ae7RFab1BFrWeCngeAp3Yq8P47jFC7x7zUK6pgv";
/// Hashlock and swap id of the first public swap (2026-07-21).
const KNOWN_HASHLOCK: &str = "ecca22673f2bba423a5689cfaf6d3f6d34dc076a57a8747b21f64714e06cee21";
const KNOWN_SWAP_ID: &str = "ebffdf016e5940338f53bafad13da936d44fd3a1c6957bc8f4c06594bc6247d4";

fn sequencer() -> sequencer_service_rpc::SequencerClient {
    SequencerClientBuilder::default()
        .build(url::Url::parse(DEFAULT_SEQUENCER_URL).unwrap())
        .expect("client")
}

#[tokio::test]
#[ignore = "network: public LEZ testnet"]
async fn fingerprint_matches_public_testnet() {
    let report = run_fingerprint(&sequencer(), DEFAULT_SEQUENCER_URL)
        .await
        .expect("getProgramIds should succeed");
    println!(
        "fingerprint report: {}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    assert!(
        report.matched,
        "public testnet ImageIDs must match the embedded v0.2.0 pin; mismatches: {:?}",
        report.mismatches
    );
    assert_eq!(report.programs.len(), 5);
}

#[tokio::test]
#[ignore = "network: public LEZ testnet"]
async fn balance_of_known_account_reads() {
    let account = parse_base58_account_id(KNOWN_ACCOUNT).unwrap();
    let balance = sequencer()
        .get_account_balance(account)
        .await
        .expect("get_account_balance should succeed");
    println!("balance of {KNOWN_ACCOUNT}: {balance}");
    // The first-swap maker settled at 650 LEZ; it may move, but the account
    // exists and the read must succeed.
}

#[tokio::test]
#[ignore = "network: Sepolia via publicnode"]
async fn sepolia_htlc_readable_by_swap_id_and_hashlock() {
    let eth = EthReader::connect(
        DEFAULT_ETH_RPC_URL,
        DEFAULT_ETH_HTLC_ADDRESS,
        DEFAULT_ETH_HTLC_FROM_BLOCK,
    )
    .await
    .expect("ws connect");

    // By swap id: the first public swap is CLAIMED.
    let swap_id = parse_bytes32(KNOWN_SWAP_ID).unwrap();
    let htlc = eth.htlc_by_swap_id(swap_id).await.expect("getHTLC");
    println!(
        "swap {KNOWN_SWAP_ID}: state={} amount_wei={} hashlock=0x{}",
        state_str(&htlc.state),
        htlc.amount,
        hex::encode(htlc.hashlock.0)
    );
    assert_eq!(state_str(&htlc.state), "CLAIMED");
    assert_eq!(hex::encode(htlc.hashlock.0), KNOWN_HASHLOCK);

    // By hashlock: the Locked-event scan resolves to the same swap id.
    let hashlock = parse_bytes32(KNOWN_HASHLOCK).unwrap();
    let (found, from, to) = eth.find_by_hashlock(hashlock).await.expect("scan");
    println!("hashlock scan over blocks {from}..{to}: {} match(es)", found.len());
    assert!(
        found.iter().any(|f| hex::encode(f.swap_id) == KNOWN_SWAP_ID),
        "expected swap id {KNOWN_SWAP_ID} in scan results"
    );
}
