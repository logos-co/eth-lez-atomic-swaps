use std::time::Duration;

use lez_htlc_program::HTLCState;
use nssa_core::account::AccountId;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::{error::Result, lez::client::LezClient};

#[derive(Debug, Clone)]
pub enum LezHtlcEvent {
    Locked {
        escrow_pda: AccountId,
    },
    Claimed {
        escrow_pda: AccountId,
        preimage: Vec<u8>,
    },
    Refunded {
        escrow_pda: AccountId,
    },
}

/// Poll the escrow PDA for state transitions and forward events to `tx`.
///
/// Stops after the escrow reaches a terminal state (Claimed or Refunded)
/// or when the receiver is dropped.
pub async fn watch_escrow(
    client: &LezClient,
    hashlock: [u8; 32],
    poll_interval: Duration,
    tx: mpsc::Sender<LezHtlcEvent>,
) -> Result<()> {
    let pda = client.escrow_pda(&hashlock);
    let mut last_state: Option<HTLCState> = None;

    loop {
        match client.get_escrow(&hashlock).await {
            Ok(Some(escrow)) => {
                let current = escrow.state;

                // For Locked state, verify the PDA actually has funds.
                // The sequencer may return phantom data for non-existent PDAs.
                if current == HTLCState::Locked && last_state.is_none() {
                    let balance = client.get_balance(&pda).await.unwrap_or(0);
                    if balance == 0 {
                        debug!("escrow PDA has zero balance, treating as non-existent");
                        tokio::time::sleep(poll_interval).await;
                        continue;
                    }
                }

                if last_state != Some(current) {
                    debug!(?current, "LEZ escrow state changed");
                    let event = match current {
                        HTLCState::Locked => LezHtlcEvent::Locked {
                            escrow_pda: pda,
                        },
                        HTLCState::Claimed => LezHtlcEvent::Claimed {
                            escrow_pda: pda,
                            preimage: escrow.preimage.unwrap_or_default(),
                        },
                        HTLCState::Refunded => LezHtlcEvent::Refunded {
                            escrow_pda: pda,
                        },
                    };

                    if tx.send(event).await.is_err() {
                        return Ok(());
                    }

                    last_state = Some(current);

                    // Terminal states — stop polling.
                    if matches!(current, HTLCState::Claimed | HTLCState::Refunded) {
                        return Ok(());
                    }
                }
            }
            Ok(None) => {
                debug!("escrow PDA not found yet, retrying");
            }
            Err(e) => {
                warn!(%e, "transient error polling escrow, will retry");
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}
