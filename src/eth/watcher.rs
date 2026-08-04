use alloy::primitives::{Address, FixedBytes};
use alloy::providers::Provider;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::client::EthHTLC;
use crate::error::Result;

#[derive(Debug, Clone)]
pub enum EthHtlcEvent {
    Locked {
        swap_id: FixedBytes<32>,
        sender: Address,
        recipient: Address,
        amount: alloy::primitives::U256,
        hashlock: FixedBytes<32>,
        timelock: alloy::primitives::U256,
        /// The taker's LEZ AccountId, published by the taker in its own ETH
        /// lock. This is the ONLY account that can claim the counterpart LEZ
        /// escrow (`execute_claim` asserts `taker.account_id ==
        /// escrow.taker_id`), so the maker must lock to exactly this value —
        /// never to a static configured account.
        taker_lez_account: FixedBytes<32>,
    },
    Claimed {
        swap_id: FixedBytes<32>,
        preimage: [u8; 32],
    },
    Refunded {
        swap_id: FixedBytes<32>,
    },
}

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4);
const REPLAY_BLOCKS: u64 = 256;

/// Watch EthHTLC events by polling `eth_getLogs` and forward them to `tx`.
///
/// Replays historical events from the last 256 blocks on startup, then polls
/// incrementally. Polling `eth_getLogs` (rather than `watch()`/`subscribe()`)
/// is deliberate: public RPC providers aggressively expire filter IDs and
/// pubsub subscriptions, which silently kills stream-based watchers. Log
/// queries are stateless and survive provider restarts/load-balancing.
pub async fn watch_events(
    client: &super::client::EthClient,
    tx: mpsc::Sender<EthHtlcEvent>,
) -> Result<()> {
    let contract = EthHTLC::new(client.contract_address(), client.provider().clone());
    let provider = client.provider();

    // Retry the initial lookup rather than defaulting to 0: a genesis-anchored
    // from_block makes every subsequent eth_getLogs span the whole chain, which
    // public RPCs reject — wedging the watcher permanently.
    let current_block = loop {
        match provider.get_block_number().await {
            Ok(b) => break b,
            Err(e) => {
                warn!(%e, "transient error fetching initial block number, will retry");
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    };
    let mut from_block = current_block.saturating_sub(REPLAY_BLOCKS);

    loop {
        let latest = match provider.get_block_number().await {
            Ok(b) => b,
            Err(e) => {
                warn!(%e, "transient error fetching block number, will retry");
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }
        };

        if latest >= from_block {
            // Locked
            match contract
                .Locked_filter()
                .from_block(from_block)
                .to_block(latest)
                .query()
                .await
            {
                Ok(events) => {
                    for (event, _) in events {
                        debug!(swap_id = %event.swapId, "Locked event");
                        let ev = EthHtlcEvent::Locked {
                            swap_id: event.swapId,
                            sender: event.sender,
                            recipient: event.recipient,
                            amount: event.amount,
                            hashlock: event.hashlock,
                            timelock: event.timelock,
                            taker_lez_account: event.takerLezAccount,
                        };
                        if tx.send(ev).await.is_err() {
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    warn!(%e, "transient error querying Locked logs, will retry");
                    tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
            }

            // Claimed
            match contract
                .Claimed_filter()
                .from_block(from_block)
                .to_block(latest)
                .query()
                .await
            {
                Ok(events) => {
                    for (event, _) in events {
                        debug!(swap_id = %event.swapId, "Claimed event");
                        let ev = EthHtlcEvent::Claimed {
                            swap_id: event.swapId,
                            preimage: event.preimage.into(),
                        };
                        if tx.send(ev).await.is_err() {
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    warn!(%e, "transient error querying Claimed logs, will retry");
                    tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
            }

            // Refunded
            match contract
                .Refunded_filter()
                .from_block(from_block)
                .to_block(latest)
                .query()
                .await
            {
                Ok(events) => {
                    for (event, _) in events {
                        debug!(swap_id = %event.swapId, "Refunded event");
                        let ev = EthHtlcEvent::Refunded {
                            swap_id: event.swapId,
                        };
                        if tx.send(ev).await.is_err() {
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    warn!(%e, "transient error querying Refunded logs, will retry");
                    tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
            }

            // All three ranges succeeded — advance the cursor.
            from_block = latest + 1;
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
