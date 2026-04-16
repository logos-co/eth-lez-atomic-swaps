//! Integration tests for the embedded Logos messaging client.
//!
//! Self-contained: each test spawns its own 2-node mesh in-process. No
//! external infra required — just `cargo test --test messaging_integration --
//! --test-threads=1` (serial because libwaku binds TCP ports and the
//! callback global state is process-wide).

use std::time::Duration;

use serial_test::serial;
use swap_orchestrator::messaging::client::MessagingClient;
use swap_orchestrator::messaging::node::MessagingNodeConfig;
use swap_orchestrator::messaging::topics::OFFERS_TOPIC;
use swap_orchestrator::messaging::types::SwapOffer;

fn test_offer() -> SwapOffer {
    SwapOffer {
        hashlock: "aa".repeat(32),
        lez_amount: 1000,
        eth_amount: 2000,
        maker_eth_address: "0x1234567890abcdef1234567890abcdef12345678".into(),
        maker_lez_account: base58::ToBase58::to_base58([0xBBu8; 32].as_slice()),
        lez_timelock: 9999999999,
        eth_timelock: 9999999999,
        lez_htlc_program_id: "cc".repeat(32),
        eth_htlc_address: "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd".into(),
    }
}

/// Spawn two nodes (port 0 = OS-assigned to avoid collisions on
/// repeated runs — see delivery-dogfooding.md #3) and connect them.
async fn two_node_mesh() -> (MessagingClient, MessagingClient) {
    let a = MessagingClient::spawn(MessagingNodeConfig {
        listen_port: 0,
        node_key_path: None,
        bootstrap_peers: vec![],
    })
    .await
    .expect("node A spawn");

    let a_addrs = a.listen_addresses().await.expect("listen_addresses A");
    let a_addr = a_addrs.into_iter().next().expect("A has listen addr");

    let b = MessagingClient::spawn(MessagingNodeConfig {
        listen_port: 0,
        node_key_path: None,
        bootstrap_peers: vec![a_addr.to_string()],
    })
    .await
    .expect("node B spawn");

    a.subscribe(&[OFFERS_TOPIC]).await.expect("A subscribe");
    b.subscribe(&[OFFERS_TOPIC]).await.expect("B subscribe");

    // Let gossipsub mesh form before publishing.
    tokio::time::sleep(Duration::from_secs(2)).await;

    (a, b)
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn relay_roundtrip() {
    let (a, b) = two_node_mesh().await;

    let offer = test_offer();
    a.publish(OFFERS_TOPIC, &offer).await.unwrap();

    // Brief pause for relay propagation.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let received: Vec<SwapOffer> = b.poll_messages(OFFERS_TOPIC).await.unwrap();
    assert!(!received.is_empty(), "expected at least one offer");

    let first = &received[0];
    assert_eq!(first.hashlock, offer.hashlock);
    assert_eq!(first.lez_amount, offer.lez_amount);
    assert_eq!(first.eth_amount, offer.eth_amount);

    let _ = a.shutdown().await;
    let _ = b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn store_query_returns_empty_stub() {
    // The bindings don't expose a way to enable the store protocol
    // server-side, so store_query is a no-op stub that always returns
    // empty. See delivery-dogfooding.md #12.
    let (a, b) = two_node_mesh().await;

    let entries = b
        .store_query(&[OFFERS_TOPIC], None, Some(10))
        .await
        .expect("store_query");
    assert!(entries.is_empty(), "store_query stub should return empty");

    let _ = a.shutdown().await;
    let _ = b.shutdown().await;
}
