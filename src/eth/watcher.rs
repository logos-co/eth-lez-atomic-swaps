use alloy::primitives::{Address, FixedBytes};
use futures_util::StreamExt;
use tokio::sync::mpsc;

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
pub async fn watch_events(
    client: &super::client::EthClient,
    tx: mpsc::Sender<EthHtlcEvent>,
) -> Result<()> {
    let contract = EthHTLC::new(client.contract_address(), client.provider().clone());

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
