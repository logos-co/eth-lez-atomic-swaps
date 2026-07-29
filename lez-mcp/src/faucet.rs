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

/// Solve the current challenge and submit one claim transaction crediting
/// `winner`. Returns the tx hash; the caller is responsible for waiting until
/// the claim commits (winner balance +150 / seed rotation) before re-claiming.
///
/// `pow_permits` is a process-global concurrency guard: only a small number of
/// PoW solves run at once, so concurrent/multi-claim callers can never occupy
/// every blocking worker. Each solve is additionally bounded by
/// [`MAX_POW_ITERATIONS`] and [`POW_SOLVE_TIMEOUT`].
pub async fn submit_claim(
    sequencer: &SequencerClient,
    winner: AccountId,
    pow_permits: &Semaphore,
) -> Result<ClaimSubmission, String> {
    let challenge = pinata_challenge(sequencer).await?;

    // Serialize CPU-bound solves behind the global guard, and give the
    // uncancellable blocking task a deadline + cancel flag so it self-limits.
    let _permit = pow_permits
        .acquire()
        .await
        .map_err(|e| format!("PoW concurrency guard closed: {e}"))?;
    let deadline = Instant::now() + POW_SOLVE_TIMEOUT;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_task = cancel.clone();
    let solution = tokio::task::spawn_blocking(move || {
        compute_solution_bounded(&challenge, MAX_POW_ITERATIONS, Some(deadline), &cancel_task)
    })
    .await
    .map_err(|e| {
        // The blocking task was dropped/panicked — signal it to stop hashing.
        cancel.store(true, Ordering::Relaxed);
        format!("PoW task panicked: {e}")
    })??;

    let instruction_data = Program::serialize_instruction(solution)
        .map_err(|e| format!("failed to serialize pinata instruction: {e}"))?;

    // Unsigned public transaction: accounts [pinata, winner], no nonces, no
    // witnesses — matches the wallet's PublicNoSign claim path exactly.
    let message = Message::new_preserialized(
        programs::pinata().id(),
        vec![system_accounts::pinata_account_id(), winner],
        vec![],
        instruction_data,
    );
    let tx = PublicTransaction::new(message, WitnessSet::from_raw_parts(vec![]));

    let tx_hash = sequencer
        .send_transaction(LeeTransaction::Public(tx))
        .await
        .map_err(|e| format!("pinata claim submission failed: {e}"))?;

    Ok(ClaimSubmission {
        solution,
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
    fn pinata_account_id_is_the_canonical_system_account() {
        use swap_orchestrator::config::account_id_to_base58;
        assert_eq!(
            account_id_to_base58(&system_accounts::pinata_account_id()),
            "EfQhKQAkX2FJiwNii2WFQsGndjvF1Mzd7RuVe7QdPLw7"
        );
    }
}
