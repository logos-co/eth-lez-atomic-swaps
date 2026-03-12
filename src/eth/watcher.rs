use alloy::primitives::{Address, FixedBytes};
use alloy::providers::Provider;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::debug;

use super::client::EthHTLC;
use crate::error::{Result, SwapError};

#[derive(Debug, Clone)]
pub enum EthHtlcEvent {
    Locked {
        swap_id: FixedBytes<32>,
        sender: Address,
        recipient: Address,
        amount: alloy::primitives::U256,
        hashlock: FixedBytes<32>,
        timelock: alloy::primitives::U256,
    },
    Claimed {
        swap_id: FixedBytes<32>,
        preimage: [u8; 32],
    },
    Refunded {
        swap_id: FixedBytes<32>,
    },
}

/// Subscribe to all EthHTLC events via WebSocket and forward them to `tx`.
///
/// Replays historical Locked events from the last 256 blocks before subscribing
/// to new events, so events emitted before the watcher started are not missed.
pub async fn watch_events(
    client: &super::client::EthClient,
    tx: mpsc::Sender<EthHtlcEvent>,
) -> Result<()> {
    let contract = EthHTLC::new(client.contract_address(), client.provider().clone());

    // Replay recent Locked events so we don't miss locks that happened before
    // the watcher started. Query from (current - 256) to latest.
    let current_block = client
        .provider()
        .get_block_number()
        .await
        .unwrap_or(0);
    let from_block = current_block.saturating_sub(256);

    let historical_locked = contract
        .Locked_filter()
        .from_block(from_block)
        .query()
        .await
        .unwrap_or_default();

    for (event, _) in &historical_locked {
        debug!(swap_id = %event.swapId, "replaying historical Locked event");
        let ev = EthHtlcEvent::Locked {
            swap_id: event.swapId,
            sender: event.sender,
            recipient: event.recipient,
            amount: event.amount,
            hashlock: event.hashlock,
            timelock: event.timelock,
        };
        if tx.send(ev).await.is_err() {
            return Ok(());
        }
    }

    // Now subscribe to new events going forward.
    let locked = contract
        .Locked_filter()
        .watch()
        .await
        .map_err(|e| SwapError::EthRpc(format!("subscribe Locked failed: {e}")))?;

    let claimed = contract
        .Claimed_filter()
        .watch()
        .await
        .map_err(|e| SwapError::EthRpc(format!("subscribe Claimed failed: {e}")))?;

    let refunded = contract
        .Refunded_filter()
        .watch()
        .await
        .map_err(|e| SwapError::EthRpc(format!("subscribe Refunded failed: {e}")))?;

    let mut locked_stream = locked.into_stream();
    let mut claimed_stream = claimed.into_stream();
    let mut refunded_stream = refunded.into_stream();

    loop {
        tokio::select! {
            Some(log) = locked_stream.next() => {
                if let Ok((event, _)) = log {
                    let ev = EthHtlcEvent::Locked {
                        swap_id: event.swapId,
                        sender: event.sender,
                        recipient: event.recipient,
                        amount: event.amount,
                        hashlock: event.hashlock,
                        timelock: event.timelock,
                    };
                    if tx.send(ev).await.is_err() { return Ok(()); }
                }
            }
            Some(log) = claimed_stream.next() => {
                if let Ok((event, _)) = log {
                    let ev = EthHtlcEvent::Claimed {
                        swap_id: event.swapId,
                        preimage: event.preimage.into(),
                    };
                    if tx.send(ev).await.is_err() { return Ok(()); }
                }
            }
            Some(log) = refunded_stream.next() => {
                if let Ok((event, _)) = log {
                    let ev = EthHtlcEvent::Refunded {
                        swap_id: event.swapId,
                    };
                    if tx.send(ev).await.is_err() { return Ok(()); }
                }
            }
            else => break,
        }
    }

    Ok(())
}
