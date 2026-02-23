use alloy::primitives::U256;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    config::SwapConfig,
    error::Result,
    eth::client::EthClient,
    eth::watcher::{self, EthHtlcEvent},
    lez::client::LezClient,
    swap::{refund::now_unix, types::SwapOutcome},
};

/// Run the maker side of an atomic swap.
///
/// The maker generates a secret preimage, locks LEZ first, waits for the
/// taker to lock ETH, then claims ETH (revealing the preimage on-chain).
pub async fn run_maker(
    config: &SwapConfig,
    eth_client: &EthClient,
    lez_client: &LezClient,
) -> Result<SwapOutcome> {
    // 1. Generate random preimage and compute hashlock.
    let preimage: [u8; 32] = rand::random();
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();
    info!(hashlock = hex::encode(hashlock), "maker: generated preimage");

    // 2. Lock LEZ.
    let lez_lock_tx = lez_client
        .lock(hashlock, config.counterparty_lez_account_id, config.lez_amount)
        .await?;
    info!(tx_hash = %lez_lock_tx, "maker: LEZ locked");

    // 3. Watch for ETH Locked event from the taker.
    let (tx, mut rx) = mpsc::channel::<EthHtlcEvent>(16);
    let watcher_eth_client = EthClient::new(config).await?;
    let watcher_handle = tokio::spawn(async move {
        let _ = watcher::watch_events(&watcher_eth_client, tx).await;
    });

    let swap_id = loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                if let EthHtlcEvent::Locked {
                    swap_id,
                    recipient,
                    amount,
                    hashlock: event_hashlock,
                    ..
                } = event
                {
                    // Match: correct hashlock, maker is the recipient, sufficient amount.
                    if event_hashlock.0 == hashlock
                        && recipient == config.counterparty_eth_address
                        && amount >= U256::from(config.eth_amount)
                    {
                        info!(%swap_id, "maker: matched ETH Locked event");
                        break swap_id;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(
                config.lez_timelock.saturating_sub(now_unix())
            )) => {
                // LEZ timelock expired — abort and refund.
                watcher_handle.abort();
                info!("maker: LEZ timelock expired, refunding");
                let lez_refund_tx = lez_client.refund(&hashlock).await.ok();
                return Ok(SwapOutcome::Refunded {
                    eth_refund_tx: None,
                    lez_refund_tx,
                });
            }
        }
    };

    watcher_handle.abort();

    // 4. Claim ETH by revealing the preimage.
    let eth_claim_tx = eth_client.claim(swap_id, preimage).await?;
    info!(%eth_claim_tx, "maker: ETH claimed");

    Ok(SwapOutcome::Completed {
        preimage,
        eth_claim_tx,
        lez_claim_tx: lez_lock_tx,
    })
}
