use alloy::primitives::U256;
use lee::AccountId;
use lez_htlc_program::{HTLCEscrow, HTLCState};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    config::SwapConfig,
    error::{Result, SwapError},
    eth::client::EthClient,
    lez::client::LezClient,
    lez::watcher::{self, LezHtlcEvent},
    swap::{
        progress::{self, ProgressSender, SwapProgress},
        refund::now_unix,
        types::SwapOutcome,
    },
};

/// Minimum time (in seconds) the LEZ escrow's timelock must still have to run
/// before the taker is willing to claim. LEZ claim confirmation can take up to
/// ~300s on the public testnet; claiming with less headroom risks the maker
/// refunding LEZ the instant our claim reveals the preimage on-chain and then
/// sweeping our still-locked ETH (a poisoned/expired LEZ timelock). Chosen to
/// match the maker's own default `TIMELOCK_MARGIN_MINUTES` (5 min).
const LEZ_CLAIM_MARGIN_SECS: u64 = 300;

/// Run the taker side of an atomic swap (taker-locks-first).
///
/// The taker generates a secret preimage, locks ETH first (long timelock),
/// waits for the maker to lock LEZ (short timelock), then claims LEZ
/// (revealing the preimage on the LEZ chain).
///
/// If `override_preimage` is `Some`, uses it instead of generating a random one.
/// This is useful for testing/demo where determinism is needed.
pub async fn run_taker(
    config: &SwapConfig,
    eth_client: &EthClient,
    lez_client: &LezClient,
    override_preimage: Option<[u8; 32]>,
    progress: Option<ProgressSender>,
) -> Result<SwapOutcome> {
    // 1. Generate preimage and compute hashlock.
    let preimage: [u8; 32] = override_preimage.unwrap_or_else(rand::random);
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();
    info!(
        hashlock = hex::encode(hashlock),
        "taker: generated preimage"
    );
    progress::report(
        &progress,
        SwapProgress::PreimageGenerated {
            hashlock: hex::encode(hashlock),
        },
    );

    // 2. Lock ETH (long timelock), publishing OUR LEZ account on-chain so the
    // maker knows which account to name as the sole claimant of its LEZ escrow.
    // This is per-swap and self-declared: no out-of-band agreement, and no
    // static `LEZ_TAKER_ACCOUNT_ID` on the maker's side.
    progress::report(&progress, SwapProgress::LockingEth);
    let lock_receipt = eth_client
        .lock(
            hashlock,
            config.eth_timelock,
            config.eth_recipient_address,
            *lez_client.account_id().value(),
            U256::from(config.eth_amount),
        )
        .await?;
    let swap_id = lock_receipt.swap_id;
    info!(%swap_id, tx_hash = %lock_receipt.tx_hash, "taker: ETH locked");
    progress::report(
        &progress,
        SwapProgress::EthLocked {
            swap_id: format!("{swap_id}"),
            tx_hash: format!("{}", lock_receipt.tx_hash),
            chain_id: eth_client.chain_id(),
        },
    );

    // 3. Watch for LEZ escrow lock from maker.
    progress::report(&progress, SwapProgress::WaitingForLezLock);
    let (tx, mut rx) = mpsc::channel::<LezHtlcEvent>(16);
    let watcher_lez_client = LezClient::new(config)?;
    let poll_interval = config.poll_interval;
    let watcher_handle = tokio::spawn(async move {
        let _ = watcher::watch_escrow(&watcher_lez_client, hashlock, poll_interval, tx).await;
    });

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    LezHtlcEvent::Locked { .. } => {
                        info!("taker: LEZ escrow locked by maker");
                        progress::report(&progress, SwapProgress::LezLockDetected);
                        break;
                    }
                    LezHtlcEvent::Refunded { .. } => {
                        // Maker refunded LEZ — swap aborted.
                        watcher_handle.abort();
                        info!("taker: maker refunded LEZ, refunding ETH");
                        progress::report(&progress, SwapProgress::Refunding);
                        let eth_refund_tx = eth_client.refund(swap_id).await.ok();
                        progress::report(&progress, SwapProgress::RefundComplete);
                        return Ok(SwapOutcome::Refunded {
                            eth_refund_tx,
                            lez_refund_tx: None,
                        });
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(
                config.eth_timelock.saturating_sub(now_unix())
            )) => {
                // ETH timelock expired — refund ETH.
                watcher_handle.abort();
                info!("taker: ETH timelock expired, maker never locked LEZ");
                progress::report(&progress, SwapProgress::TimelockExpired);
                progress::report(&progress, SwapProgress::Refunding);
                let eth_refund_tx = eth_client.refund(swap_id).await.ok();
                progress::report(&progress, SwapProgress::RefundComplete);
                return Ok(SwapOutcome::Refunded {
                    eth_refund_tx,
                    lez_refund_tx: None,
                });
            }
        }
    }

    watcher_handle.abort();

    // 4. Verify LEZ escrow params.
    progress::report(&progress, SwapProgress::VerifyingLezEscrow);
    let escrow =
        lez_client
            .get_escrow(&hashlock)
            .await?
            .ok_or_else(|| SwapError::InvalidState {
                expected: "Locked escrow".into(),
                actual: "no escrow found".into(),
            })?;

    if escrow.state != HTLCState::Locked {
        return Err(SwapError::InvalidState {
            expected: "Locked".into(),
            actual: format!("{:?}", escrow.state),
        });
    }
    if escrow.amount < config.lez_amount {
        return Err(SwapError::InvalidState {
            expected: format!("amount >= {}", config.lez_amount),
            actual: format!("amount = {}", escrow.amount),
        });
    }
    // Verify the escrow PDA actually holds funds (not a phantom account).
    let pda = lez_client.escrow_pda(&hashlock);
    let pda_balance = lez_client.get_balance(&pda).await?;
    if pda_balance < config.lez_amount {
        return Err(SwapError::InvalidState {
            expected: format!("PDA balance >= {}", config.lez_amount),
            actual: format!("PDA balance = {}", pda_balance),
        });
    }

    // P0-2 (defense-in-depth): validate the escrow's BINDING before revealing the
    // preimage. A malicious maker can lock a genuine-program escrow that is still
    // poisoned — bound to the wrong claimant, or with a short/expired LEZ timelock
    // that lets the maker refund-race us the instant our claim reveals the preimage
    // on-chain. On ANY mismatch we do NOT claim (never reveal the preimage) and
    // return an error, so the ETH auto-refunds at its own (longer) timelock.
    verify_escrow_binding(
        &escrow,
        &lez_client.account_id(),
        &hashlock,
        config.eth_timelock,
        now_unix(),
    )?;

    info!("taker: LEZ escrow verified");
    progress::report(&progress, SwapProgress::LezEscrowVerified);

    // 5. Claim LEZ (reveals preimage on the LEZ chain).
    progress::report(&progress, SwapProgress::ClaimingLez);
    let lez_claim_tx = lez_client.claim(&hashlock, &preimage).await?;
    info!(tx_hash = %lez_claim_tx, "taker: LEZ claimed");
    progress::report(
        &progress,
        SwapProgress::LezClaimed {
            tx_hash: lez_claim_tx.clone(),
        },
    );

    Ok(SwapOutcome::Completed {
        preimage,
        eth_tx: swap_id,
        lez_tx: lez_claim_tx,
    })
}

/// Pure validation of the LEZ escrow's binding, run BEFORE the taker reveals the
/// preimage (P0-2). Extracted from `run_taker` so the adversarial cases are
/// unit-testable without a live sequencer.
///
/// Returns `Err` (⇒ `run_taker` does NOT claim, so the preimage is never
/// revealed and the ETH auto-refunds) if any of these fail:
///   (a) the escrow's sole claimant (`taker_id`) is not OUR account — the one we
///       published in our ETH lock; a different `taker_id` means only the maker
///       (or their confederate) can claim, so revealing the preimage hands it over.
///   (b) the escrow is not bound to OUR `hashlock`.
///   (c) the LEZ timelock (maker's refund time; milliseconds on the wire) does
///       not leave us `LEZ_CLAIM_MARGIN_SECS` of headroom, OR does not expire
///       strictly before our own ETH timelock — a short/expired LEZ timelock is
///       the refund-race poison (the maker refunds LEZ the instant our claim
///       reveals the preimage, then sweeps our still-locked ETH).
fn verify_escrow_binding(
    escrow: &HTLCEscrow,
    our_account: &AccountId,
    hashlock: &[u8; 32],
    eth_timelock_secs: u64,
    now_secs: u64,
) -> Result<()> {
    if escrow.taker_id != *our_account {
        return Err(SwapError::InvalidState {
            expected: format!(
                "escrow taker_id == our account {}",
                hex::encode(our_account.value().as_slice())
            ),
            actual: format!(
                "escrow taker_id == {}",
                hex::encode(escrow.taker_id.value().as_slice())
            ),
        });
    }
    if escrow.hashlock != *hashlock {
        return Err(SwapError::InvalidState {
            expected: format!("escrow hashlock == {}", hex::encode(hashlock)),
            actual: format!("escrow hashlock == {}", hex::encode(escrow.hashlock)),
        });
    }
    // LEZ timelock is milliseconds on the wire; everything else here is seconds.
    let lez_expiry_secs = escrow.timelock / 1000;
    if lez_expiry_secs < now_secs.saturating_add(LEZ_CLAIM_MARGIN_SECS) {
        return Err(SwapError::InvalidState {
            expected: format!(
                "LEZ timelock >= now + {LEZ_CLAIM_MARGIN_SECS}s claim margin (now = {now_secs})"
            ),
            actual: format!("LEZ timelock = {lez_expiry_secs}"),
        });
    }
    if lez_expiry_secs >= eth_timelock_secs {
        return Err(SwapError::InvalidState {
            expected: format!("LEZ timelock < our ETH timelock ({eth_timelock_secs})"),
            actual: format!("LEZ timelock = {lez_expiry_secs}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lee_core::program::{PdaSeed, ProgramId};

    fn acct(byte: u8) -> AccountId {
        let program_id: ProgramId = [0x42u32; 8];
        AccountId::for_public_pda(&program_id, &PdaSeed::new([byte; 32]))
    }

    // A well-formed escrow bound to `me`, with a LEZ timelock that clears the
    // claim margin and sits strictly before the ETH timelock. `now = 1_000_000`,
    // ETH timelock `now + 4000`, LEZ expiry `now + 2000` (comfortably > margin,
    // < ETH). timelock is milliseconds on the wire.
    fn good_escrow(me: AccountId, hashlock: [u8; 32]) -> HTLCEscrow {
        HTLCEscrow {
            hashlock,
            maker_id: acct(0xEE),
            taker_id: me,
            amount: 150,
            state: HTLCState::Locked,
            timelock: (1_000_000 + 2_000) * 1000,
            preimage: None,
        }
    }

    const NOW: u64 = 1_000_000;
    const ETH_TL: u64 = 1_000_000 + 4_000;

    #[test]
    fn honest_escrow_passes() {
        let me = acct(0x01);
        let hl = [0xAAu8; 32];
        assert!(verify_escrow_binding(&good_escrow(me, hl), &me, &hl, ETH_TL, NOW).is_ok());
    }

    // The maker named a DIFFERENT taker as the sole claimant. Revealing the
    // preimage would hand the claim to them — must abort (no reveal).
    #[test]
    fn wrong_taker_id_aborts_without_reveal() {
        let me = acct(0x01);
        let attacker = acct(0x02);
        let hl = [0xAAu8; 32];
        let mut escrow = good_escrow(me, hl);
        escrow.taker_id = attacker;
        assert!(verify_escrow_binding(&escrow, &me, &hl, ETH_TL, NOW).is_err());
    }

    #[test]
    fn wrong_hashlock_aborts() {
        let me = acct(0x01);
        let hl = [0xAAu8; 32];
        let escrow = good_escrow(me, [0xBBu8; 32]);
        assert!(verify_escrow_binding(&escrow, &me, &hl, ETH_TL, NOW).is_err());
    }

    // A short/already-near-expiry LEZ timelock is the refund-race poison: it
    // leaves no safe margin to claim before the maker can refund LEZ.
    #[test]
    fn too_short_lez_timelock_aborts() {
        let me = acct(0x01);
        let hl = [0xAAu8; 32];
        let mut escrow = good_escrow(me, hl);
        // Only 10s of headroom — far under LEZ_CLAIM_MARGIN_SECS.
        escrow.timelock = (NOW + 10) * 1000;
        assert!(verify_escrow_binding(&escrow, &me, &hl, ETH_TL, NOW).is_err());
    }

    // A LEZ timelock at/after our OWN ETH timelock inverts the ordering: the
    // maker could refund-race after the preimage is revealed.
    #[test]
    fn lez_timelock_not_before_eth_aborts() {
        let me = acct(0x01);
        let hl = [0xAAu8; 32];
        let mut escrow = good_escrow(me, hl);
        escrow.timelock = ETH_TL * 1000; // equal to ETH timelock ⇒ rejected
        assert!(verify_escrow_binding(&escrow, &me, &hl, ETH_TL, NOW).is_err());
        escrow.timelock = (ETH_TL + 1_000) * 1000; // beyond ETH ⇒ rejected
        assert!(verify_escrow_binding(&escrow, &me, &hl, ETH_TL, NOW).is_err());
    }
}
