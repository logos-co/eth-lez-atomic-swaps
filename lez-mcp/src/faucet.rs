//! Native pinata (faucet) claims — no wallet-CLI shelling.
//!
//! The public pinata claim is a proof-of-work: find a `u128` solution such
//! that the leftmost `difficulty` bytes of `SHA256(seed || solution_le)` are
//! zero, where `difficulty = data[0]` and `seed = data[1..33]` of the pinata
//! system account. The claim transaction needs NO signatures — both accounts
//! ride as unsigned public accounts (empty nonces, empty witness set) and the
//! pinata program credits the winner 150 LEZ. The seed rotates on every
//! committed claim, so repeated claims must wait for commitment in between.
//!
//! Mirrors `wallet::program_facades::pinata::Pinata::claim` +
//! `wallet/src/cli/programs/pinata.rs::compute_solution` at LEZ v0.2.0.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lee::{
    AccountId, PublicTransaction,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_protocol::LeeTransaction;
use sequencer_service_rpc::{RpcClient as _, SequencerClient};
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;

/// Sets a shared cancel flag when dropped. Held on the async side across the
/// `spawn_blocking` join: if the awaiting future is cancelled (the MCP request
/// is dropped), this guard's `Drop` fires and flips the flag, so the detached
/// blocking solver stops promptly at its next check instead of hashing on.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Prize per claim, hardcoded in the pinata guest program.
pub const PRIZE_PER_CLAIM: u128 = 150;

/// Hard iteration ceiling for a single PoW solve. The public-testnet
/// difficulty is a handful of leading zero *bytes*; a genuine solution is
/// found well within this bound. The cap turns a mis-configured/hostile
/// difficulty (which would otherwise spin a blocking worker forever) into a
/// prompt, recoverable error.
pub const MAX_POW_ITERATIONS: u128 = 1 << 32;

/// Wall-clock budget for a single PoW solve. Bounds the uncancellable
/// `spawn_blocking` task so it cannot occupy a worker indefinitely.
pub const POW_SOLVE_TIMEOUT: Duration = Duration::from_secs(45);

/// How often the solve loop checks the deadline / cancellation flag (a cheap
/// mask, not a modulo, on the hot path).
const POW_CHECK_MASK: u128 = (1 << 18) - 1;

/// Check whether the leftmost `difficulty` bytes of
/// `SHA256(seed || solution.to_le_bytes())` are zero.
pub fn validate_solution(difficulty: u8, seed: &[u8; 32], solution: u128) -> bool {
    let mut bytes = [0u8; 32 + 16];
    bytes[..32].copy_from_slice(seed);
    bytes[32..].copy_from_slice(&solution.to_le_bytes());
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    digest[..usize::from(difficulty)].iter().all(|&b| b == 0)
}

/// Brute-force the PoW solution for the pinata account's 33-byte challenge
/// data (`[difficulty, seed[0..32]]`). CPU-bound: run under spawn_blocking.
///
/// Unbounded — retained for the pure-algorithm unit tests. Production callers
/// use [`compute_solution_bounded`], which enforces work/time/cancellation
/// caps so a hostile difficulty cannot pin a blocking worker.
pub fn compute_solution(data: &[u8; 33]) -> Result<u128, String> {
    let cancel = AtomicBool::new(false);
    compute_solution_bounded(data, MAX_POW_ITERATIONS, None, &cancel)
}

/// Bounded PoW solve. Stops (with an error) when any of these trip:
/// - `max_iterations` candidate solutions have been tried,
/// - `deadline` (if given) has passed,
/// - `cancel` has been set by the async side.
///
/// The deadline / cancel flag are polled every `POW_CHECK_MASK + 1` iterations
/// to keep the hot loop tight while still terminating promptly (~microseconds
/// of hashing between checks).
pub fn compute_solution_bounded(
    data: &[u8; 33],
    max_iterations: u128,
    deadline: Option<Instant>,
    cancel: &AtomicBool,
) -> Result<u128, String> {
    let difficulty = data[0];
    if difficulty > 32 {
        return Err(format!("invalid pinata difficulty {difficulty}"));
    }
    let seed: [u8; 32] = data[1..].try_into().expect("32 bytes");

    let mut solution = 0u128;
    let mut iterations = 0u128;
    while !validate_solution(difficulty, &seed, solution) {
        if iterations >= max_iterations {
            return Err(format!(
                "faucet PoW gave up after {max_iterations} iterations (difficulty {difficulty}) \
                 without a solution — the challenge may be mis-configured"
            ));
        }
        if iterations & POW_CHECK_MASK == 0 {
            if cancel.load(Ordering::Relaxed) {
                return Err("faucet PoW cancelled".to_string());
            }
            if let Some(dl) = deadline
                && Instant::now() >= dl
            {
                return Err(format!(
                    "faucet PoW exceeded its {}s time budget (difficulty {difficulty}) — stopping",
                    POW_SOLVE_TIMEOUT.as_secs()
                ));
            }
        }
        iterations += 1;
        solution = solution
            .checked_add(1)
            .ok_or_else(|| "PoW solution overflowed u128".to_string())?;
    }
    Ok(solution)
}

/// Fetch the pinata account's 33-byte challenge data.
pub async fn pinata_challenge(sequencer: &SequencerClient) -> Result<[u8; 33], String> {
    let pinata_id = system_accounts::pinata_account_id();
    let account = sequencer
        .get_account(pinata_id)
        .await
        .map_err(|e| format!("getAccount(pinata) failed: {e}"))?;
    let data: Vec<u8> = account.data.into();
    data.as_slice()
        .try_into()
        .map_err(|_| format!("unexpected pinata account data length {}", data.len()))
}

pub struct ClaimSubmission {
    pub solution: u128,
    pub tx_hash: String,
}

/// A solved-but-not-yet-submitted claim: the PoW answer for the challenge that
/// was current when [`solve_claim`] ran, plus the winner it credits.
pub struct SolvedClaim {
    pub solution: u128,
    pub winner: AccountId,
}

/// Fetch the current challenge and brute-force its PoW, crediting `winner`.
///
/// This is the CPU-bound half of a claim, kept separate from submission so the
/// caller can re-verify the write gate in the gap between the (possibly
/// 45-second) solve and the actual `send_transaction`.
///
/// Concurrency/cancellation correctness:
/// - `pow_permits` (an `Arc<Semaphore>`) caps concurrent solves. The permit is
///   acquired *owned* and **moved into the blocking closure**, so it is held
///   for exactly as long as the solve actually runs — even if the awaiting
///   future is cancelled and the `JoinHandle` is dropped (which detaches, but
///   does not stop, the blocking task). This keeps the permit count honest and
///   prevents unbounded concurrent solves under MCP cancellation.
/// - A [`CancelOnDrop`] guard is held across the join; if the future is
///   cancelled it flips the shared flag so the detached solver stops promptly.
///
/// Each solve is additionally bounded by [`MAX_POW_ITERATIONS`] and
/// [`POW_SOLVE_TIMEOUT`].
pub async fn solve_claim(
    sequencer: &SequencerClient,
    winner: AccountId,
    pow_permits: Arc<Semaphore>,
) -> Result<SolvedClaim, String> {
    let challenge = pinata_challenge(sequencer).await?;

    let permit = pow_permits
        .acquire_owned()
        .await
        .map_err(|e| format!("PoW concurrency guard closed: {e}"))?;
    let deadline = Instant::now() + POW_SOLVE_TIMEOUT;
    let cancel = Arc::new(AtomicBool::new(false));
    // Drop-guard: if THIS future is cancelled while awaiting the join below,
    // its Drop sets the flag and the detached solver stops at its next check.
    let cancel_guard = CancelOnDrop(cancel.clone());
    let cancel_task = cancel.clone();
    let solution = tokio::task::spawn_blocking(move || {
        // The permit lives here — released only when the solve genuinely ends,
        // not when the async side's future is dropped.
        let _permit = permit;
        compute_solution_bounded(&challenge, MAX_POW_ITERATIONS, Some(deadline), &cancel_task)
    })
    .await
    .map_err(|e| {
        // The blocking task panicked — signal it (redundantly) to stop.
        cancel.store(true, Ordering::Relaxed);
        format!("PoW task panicked: {e}")
    })??;

    // Solve completed normally; the guard has done its job. Defuse it so it does
    // not set the (now meaningless) flag on scope exit.
    std::mem::forget(cancel_guard);

    Ok(SolvedClaim { solution, winner })
}

/// Build and broadcast the unsigned claim transaction for an already-solved
/// challenge. Returns the tx hash; the caller must wait for on-chain commitment
/// (winner balance +150 / seed rotation) before re-claiming.
///
/// Call this immediately after a fresh write-gate check: the send happens here,
/// so the gate is re-verified after the PoW solve and after any semaphore wait.
pub async fn submit_solved_claim(
    sequencer: &SequencerClient,
    solved: &SolvedClaim,
) -> Result<ClaimSubmission, String> {
    let instruction_data = Program::serialize_instruction(solved.solution)
        .map_err(|e| format!("failed to serialize pinata instruction: {e}"))?;

    // Unsigned public transaction: accounts [pinata, winner], no nonces, no
    // witnesses — matches the wallet's PublicNoSign claim path exactly.
    let message = Message::new_preserialized(
        programs::pinata().id(),
        vec![system_accounts::pinata_account_id(), solved.winner],
        vec![],
        instruction_data,
    );
    let tx = PublicTransaction::new(message, WitnessSet::from_raw_parts(vec![]));

    let tx_hash = sequencer
        .send_transaction(LeeTransaction::Public(tx))
        .await
        .map_err(|e| format!("pinata claim submission failed: {e}"))?;

    Ok(ClaimSubmission {
        solution: solved.solution,
        tx_hash: tx_hash.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difficulty_zero_accepts_solution_zero() {
        let mut data = [0u8; 33];
        data[0] = 0;
        assert_eq!(compute_solution(&data).unwrap(), 0);
    }

    #[test]
    fn difficulty_one_solution_matches_reference_algorithm() {
        // difficulty 1, seed = all 0x11 — brute force is ~256 hashes.
        let mut data = [0x11u8; 33];
        data[0] = 1;
        let seed: [u8; 32] = data[1..].try_into().unwrap();

        let solution = compute_solution(&data).unwrap();
        assert!(validate_solution(1, &seed, solution));

        // Reference recomputation (mirrors the guest program).
        let mut bytes = [0u8; 48];
        bytes[..32].copy_from_slice(&seed);
        bytes[32..].copy_from_slice(&solution.to_le_bytes());
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        assert_eq!(digest[0], 0);

        // And it is the SMALLEST such solution.
        for s in 0..solution {
            assert!(!validate_solution(1, &seed, s));
        }
    }

    #[test]
    fn invalid_difficulty_is_rejected() {
        let mut data = [0u8; 33];
        data[0] = 33;
        assert!(compute_solution(&data).is_err());
    }

    #[test]
    fn iteration_cap_stops_an_unsolvable_challenge() {
        // difficulty 32 (all-zero digest) is effectively unsolvable; a tiny
        // iteration cap must bail out promptly instead of spinning forever.
        let mut data = [0x11u8; 33];
        data[0] = 32;
        let cancel = AtomicBool::new(false);
        let err = compute_solution_bounded(&data, 10_000, None, &cancel)
            .expect_err("must give up under the iteration cap");
        assert!(err.contains("gave up"), "unexpected error: {err}");
    }

    #[test]
    fn deadline_stops_an_unsolvable_challenge() {
        let mut data = [0x22u8; 33];
        data[0] = 32;
        let cancel = AtomicBool::new(false);
        let past = Instant::now() - Duration::from_secs(1);
        let err = compute_solution_bounded(&data, MAX_POW_ITERATIONS, Some(past), &cancel)
            .expect_err("must give up past the deadline");
        assert!(err.contains("time budget"), "unexpected error: {err}");
    }

    #[test]
    fn cancellation_flag_stops_the_solve() {
        let mut data = [0x33u8; 33];
        data[0] = 32;
        let cancel = AtomicBool::new(true);
        let err = compute_solution_bounded(&data, MAX_POW_ITERATIONS, None, &cancel)
            .expect_err("must give up when cancelled");
        assert!(err.contains("cancelled"), "unexpected error: {err}");
    }

    #[test]
    fn bounded_solve_still_finds_easy_solutions() {
        // A solvable challenge under generous caps returns the same answer as
        // the unbounded reference.
        let mut data = [0x11u8; 33];
        data[0] = 1;
        let cancel = AtomicBool::new(false);
        let bounded = compute_solution_bounded(&data, MAX_POW_ITERATIONS, None, &cancel).unwrap();
        assert_eq!(bounded, compute_solution(&data).unwrap());
    }

    #[test]
    fn cancel_on_drop_sets_the_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let _g = CancelOnDrop(flag.clone());
            assert!(!flag.load(Ordering::Relaxed), "flag stays clear while guard is live");
        }
        assert!(flag.load(Ordering::Relaxed), "dropping the guard cancels");
    }

    // The permit must live INSIDE the blocking solve, releasing only when the
    // solve actually ends — not when the async side's future is dropped. Model
    // that here: hold an owned permit inside a blocking task, confirm no second
    // permit is available while it runs, then cancel and confirm the permit is
    // released only after the (detached-style) task finishes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permit_is_held_through_the_solve_and_released_on_cancel() {
        let sem = Arc::new(Semaphore::new(1));
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_task = cancel.clone();

        // An effectively unsolvable challenge so the solve only ends on cancel.
        let mut data = [0x11u8; 33];
        data[0] = 32;

        let permit = sem.clone().acquire_owned().await.unwrap();
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            compute_solution_bounded(&data, MAX_POW_ITERATIONS, None, &cancel_task)
        });

        // While the solve runs, the sole permit is held: no second solve admitted.
        assert!(
            sem.clone().try_acquire_owned().is_err(),
            "permit must stay held for the duration of the solve"
        );

        // Cancel (as CancelOnDrop would on future-drop) and let the task finish.
        cancel.store(true, Ordering::Relaxed);
        let res = handle.await.expect("join");
        assert!(res.is_err(), "cancelled solve returns an error");

        // Only now — after the solve genuinely ended — is the permit free.
        assert!(
            sem.try_acquire().is_ok(),
            "permit released once the blocking solve ends"
        );
    }

    #[test]
    fn pinata_account_id_is_the_canonical_system_account() {
        use swap_orchestrator::config::account_id_to_base58;
        assert_eq!(
            account_id_to_base58(&system_accounts::pinata_account_id()),
            "EfQhKQAkX2FJiwNii2WFQsGndjvF1Mzd7RuVe7QdPLw7"
        );
    }
}
