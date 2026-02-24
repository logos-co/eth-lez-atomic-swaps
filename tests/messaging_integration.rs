//! Integration tests for Logos Messaging (nwaku REST API).
//!
//! Requires a running nwaku node at `http://localhost:8645`.
//! Run: `make nwaku` then `cargo test -- --ignored`

use swap_orchestrator::messaging::client::{MessagingClient, decode_waku_payload};
use swap_orchestrator::messaging::types::{SwapOffer, OFFERS_TOPIC};

fn test_offer() -> SwapOffer {
    SwapOffer {
        hashlock: "aa".repeat(32),
        lez_amount: 1000,
        eth_amount: 2000,
        maker_eth_address: "0x1234567890abcdef1234567890abcdef12345678".into(),
        maker_lez_account: "bb".repeat(32),
        lez_timelock: 9999999999,
        eth_timelock: 9999999999,
        lez_htlc_program_id: "cc".repeat(32),
        eth_htlc_address: "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd".into(),
    }
}

#[tokio::test]
#[ignore]
async fn relay_roundtrip() {
    let client = MessagingClient::new("http://localhost:8645");

    // Subscribe to the offers topic.
    client.subscribe(&[OFFERS_TOPIC]).await.unwrap();

    // Drain any leftover messages from a previous run.
    let _: Vec<SwapOffer> = client.poll_messages(OFFERS_TOPIC).await.unwrap();

    // Publish an offer.
    let offer = test_offer();
    client.publish(OFFERS_TOPIC, &offer).await.unwrap();

    // Brief pause for relay propagation.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Poll it back.
    let received: Vec<SwapOffer> = client.poll_messages(OFFERS_TOPIC).await.unwrap();
    assert!(!received.is_empty(), "expected at least one offer");

    let first = &received[0];
    assert_eq!(first.hashlock, offer.hashlock);
    assert_eq!(first.lez_amount, offer.lez_amount);
    assert_eq!(first.eth_amount, offer.eth_amount);
}

#[tokio::test]
#[ignore]
async fn store_query_roundtrip() {
    let client = MessagingClient::new("http://localhost:8645");

    client.subscribe(&[OFFERS_TOPIC]).await.unwrap();

    // Drain relay cache.
    let _: Vec<SwapOffer> = client.poll_messages(OFFERS_TOPIC).await.unwrap();

    let offer = test_offer();
    client.publish(OFFERS_TOPIC, &offer).await.unwrap();

    // Wait for store to persist.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Query store for recent messages (last 5 min).
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let five_min_ns: i64 = 5 * 60 * 1_000_000_000;

    let entries = client
        .store_query(&[OFFERS_TOPIC], Some(now_ns - five_min_ns), Some(10))
        .await
        .unwrap();

    assert!(!entries.is_empty(), "expected at least one store entry");

    // Verify we can decode the stored payload.
    let entry = entries.last().unwrap();
    let msg = entry.message.as_ref().expect("includeData should populate message");
    let decoded: SwapOffer = decode_waku_payload(&msg.payload).unwrap();
    assert_eq!(decoded.hashlock, offer.hashlock);
}
