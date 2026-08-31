//! Shared proof-of-work challenge scheme for the in-house Sepolia drip faucet.
//!
//! One crate, two consumers, so the solver and the verifier can never drift:
//! the `eth-faucet` service issues and verifies challenges here, and `swap-ffi`
//! (the app's client) solves them here.
//!
//! # The scheme
//!
//! Deliberately the same shape as the LEZ pinata faucet the repo already ships
//! (`src/lez/faucet.rs`): find a `u128` `solution` such that
//! `SHA256(seed || solution.to_le_bytes())` starts with enough zero bits.
//! Reusing that idiom means the app's "the faucet is making my CPU work for a
//! moment" story, its progress copy, and its bounded/cancellable solver
//! ergonomics are already understood by this codebase.
//!
//! **One deliberate deviation: difficulty counts zero BITS, not zero BYTES.**
//! The pinata scheme's byte granularity multiplies the expected work by 256 per
//! step — 3 zero bytes is a few seconds and 4 is over an hour, with nothing in
//! between. A faucet wants to aim at "~30 s of a laptop core", and it wants to
//! be able to *raise* difficulty as the day's budget depletes (pk910's trick),
//! so it needs a knob it can turn by 2x rather than by 256x.
//!
//! # Why a seed the server picks
//!
//! The seed is server-issued and bound to one address for one short window, so
//! a solution cannot be:
//! - **precomputed** — the client does not know the seed until it asks;
//! - **reused** — the service consumes the challenge on the first valid claim;
//! - **transplanted** to another address — the address is inside the challenge
//!   the service looks up, so a solution earned for A is not a solution for B.
//!
//! That binding lives in the service (which stores the outstanding challenge);
//! this crate owns only the hash rule and the bounded solver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Hard iteration ceiling for a single solve. At the difficulties a faucet
/// sensibly issues (see [`MAX_DIFFICULTY_BITS`]) a genuine solution is found
/// well within this bound; the cap turns a mis-configured or hostile challenge
/// into a prompt, recoverable error instead of a pinned CPU.
pub const MAX_POW_ITERATIONS: u128 = 1 << 34;

/// The largest difficulty this crate will attempt. 32 bits is ~4.3e9 hashes —
/// already minutes of work — so anything beyond it is a mis-configuration, not
/// a challenge, and is refused before the loop starts rather than after.
pub const MAX_DIFFICULTY_BITS: u8 = 32;

/// How often the solve loop checks the deadline / cancellation flag (a cheap
/// mask, not a modulo, on the hot path). Same idiom as `src/lez/faucet.rs`.
const POW_CHECK_MASK: u128 = (1 << 18) - 1;

/// A challenge as it travels over the wire (`GET /challenge`).
///
/// `seed` is lowercase hex, no `0x` — 32 bytes. `expires_at` is a Unix
/// timestamp in seconds; the service refuses solutions that arrive after it,
/// which is what keeps an outstanding challenge from being farmed at leisure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Challenge {
    pub address: String,
    pub seed: String,
    pub difficulty_bits: u8,
    pub expires_at: u64,
}

impl Challenge {
    /// The 32-byte seed, or an error naming what was wrong with the hex.
    pub fn seed_bytes(&self) -> Result<[u8; 32], String> {
        let raw = hex::decode(self.seed.trim_start_matches("0x"))
            .map_err(|e| format!("challenge seed is not hex: {e}"))?;
        raw.as_slice()
            .try_into()
            .map_err(|_| format!("challenge seed is {} bytes, expected 32", raw.len()))
    }
}

/// Count the leading zero bits of a SHA256 digest.
fn leading_zero_bits(digest: &[u8; 32]) -> u32 {
    let mut bits = 0u32;
    for &byte in digest {
        if byte == 0 {
            bits += 8;
        } else {
            bits += byte.leading_zeros();
            break;
        }
    }
    bits
}

/// The one hash rule, shared by solver and verifier: does `solution` make
/// `SHA256(seed || solution.to_le_bytes())` start with `difficulty_bits` zeros?
///
/// `difficulty_bits == 0` accepts everything, which is what makes a zero
/// difficulty a usable "PoW off" setting for local demos.
pub fn validate_solution(seed: &[u8; 32], difficulty_bits: u8, solution: u128) -> bool {
    let mut bytes = [0u8; 32 + 16];
    bytes[..32].copy_from_slice(seed);
    bytes[32..].copy_from_slice(&solution.to_le_bytes());
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    leading_zero_bits(&digest) >= u32::from(difficulty_bits)
}

/// Bounded PoW solve. Stops (with an error) when any of these trip:
/// - `max_iterations` candidate solutions have been tried,
/// - `deadline` (if given) has passed,
/// - `cancel` has been set by the caller.
///
/// CPU-bound: run it under `spawn_blocking` (or a plain thread), never on an
/// async executor's worker. The deadline / cancel flag are polled every
/// `POW_CHECK_MASK + 1` iterations so the hot loop stays tight while still
/// stopping promptly.
pub fn compute_solution_bounded(
    seed: &[u8; 32],
    difficulty_bits: u8,
    max_iterations: u128,
    deadline: Option<Instant>,
    cancel: &AtomicBool,
) -> Result<u128, String> {
    if difficulty_bits > MAX_DIFFICULTY_BITS {
        return Err(format!(
            "faucet PoW difficulty {difficulty_bits} exceeds the {MAX_DIFFICULTY_BITS}-bit ceiling \
             — refusing to start a solve that would run for hours"
        ));
    }

    let mut solution = 0u128;
    let mut iterations = 0u128;
    while !validate_solution(seed, difficulty_bits, solution) {
        if iterations >= max_iterations {
            return Err(format!(
                "faucet PoW gave up after {max_iterations} iterations (difficulty \
                 {difficulty_bits} bits) without a solution"
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
                    "faucet PoW exceeded its time budget (difficulty {difficulty_bits} bits) — \
                     stopping"
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

/// Expected number of hashes for a difficulty, for sizing copy and defaults.
/// Exact, not a fit: each hash succeeds with probability `2^-bits`.
pub fn expected_hashes(difficulty_bits: u8) -> f64 {
    2f64.powi(i32::from(difficulty_bits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn seed_of(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    #[test]
    fn zero_difficulty_accepts_solution_zero() {
        let seed = seed_of(0xAB);
        assert!(validate_solution(&seed, 0, 0));
        let cancel = AtomicBool::new(false);
        assert_eq!(
            compute_solution_bounded(&seed, 0, MAX_POW_ITERATIONS, None, &cancel).unwrap(),
            0
        );
    }

    #[test]
    fn leading_zero_bits_counts_across_the_byte_boundary() {
        let mut digest = [0u8; 32];
        assert_eq!(leading_zero_bits(&digest), 256);
        digest[0] = 0b0000_0001;
        assert_eq!(leading_zero_bits(&digest), 7);
        digest[0] = 0b1000_0000;
        assert_eq!(leading_zero_bits(&digest), 0);
        digest[0] = 0;
        digest[1] = 0b0010_0000;
        assert_eq!(leading_zero_bits(&digest), 10);
    }

    #[test]
    fn a_solve_produces_a_solution_the_verifier_accepts() {
        // 12 bits is ~4096 hashes: instant, and enough to exercise the real
        // loop rather than the degenerate zero-difficulty path.
        let seed = seed_of(0x11);
        let cancel = AtomicBool::new(false);
        let solution =
            compute_solution_bounded(&seed, 12, MAX_POW_ITERATIONS, None, &cancel).unwrap();
        assert!(validate_solution(&seed, 12, solution));
    }

    #[test]
    fn the_solve_returns_the_smallest_solution() {
        // The verifier accepts any solution; the solver counting up from zero
        // means a client and the service can also agree on WHICH one, which is
        // what makes the round trip reproducible in a demo.
        let seed = seed_of(0x22);
        let cancel = AtomicBool::new(false);
        let solution =
            compute_solution_bounded(&seed, 10, MAX_POW_ITERATIONS, None, &cancel).unwrap();
        for candidate in 0..solution {
            assert!(!validate_solution(&seed, 10, candidate));
        }
    }

    #[test]
    fn a_solution_for_one_seed_does_not_satisfy_another() {
        // The service binds a seed to one address, so this is what stops a
        // solution earned for address A being spent on address B.
        let cancel = AtomicBool::new(false);
        let solution =
            compute_solution_bounded(&seed_of(0x33), 12, MAX_POW_ITERATIONS, None, &cancel)
                .unwrap();
        assert!(!validate_solution(&seed_of(0x44), 12, solution));
    }

    #[test]
    fn an_easier_solution_does_not_satisfy_a_harder_challenge() {
        let seed = seed_of(0x55);
        let cancel = AtomicBool::new(false);
        let easy = compute_solution_bounded(&seed, 8, MAX_POW_ITERATIONS, None, &cancel).unwrap();
        // Raising difficulty must invalidate the easy answer, or "raise the
        // difficulty as the budget depletes" would be a no-op defense.
        assert!(!validate_solution(&seed, 24, easy));
    }

    #[test]
    fn difficulty_above_the_ceiling_is_refused_without_hashing() {
        let cancel = AtomicBool::new(false);
        let err = compute_solution_bounded(
            &seed_of(0x66),
            MAX_DIFFICULTY_BITS + 1,
            MAX_POW_ITERATIONS,
            None,
            &cancel,
        )
        .expect_err("must refuse an out-of-range difficulty");
        assert!(err.contains("ceiling"), "unexpected error: {err}");
    }

    #[test]
    fn the_iteration_cap_stops_an_unsolvable_challenge() {
        let cancel = AtomicBool::new(false);
        let err = compute_solution_bounded(&seed_of(0x77), 32, 10_000, None, &cancel)
            .expect_err("must give up under the iteration cap");
        assert!(err.contains("gave up"), "unexpected error: {err}");
    }

    #[test]
    fn the_deadline_stops_an_unsolvable_challenge() {
        let cancel = AtomicBool::new(false);
        let past = Instant::now() - Duration::from_secs(1);
        let err =
            compute_solution_bounded(&seed_of(0x88), 32, MAX_POW_ITERATIONS, Some(past), &cancel)
                .expect_err("must give up past the deadline");
        assert!(err.contains("time budget"), "unexpected error: {err}");
    }

    #[test]
    fn the_cancel_flag_stops_the_solve() {
        let cancel = AtomicBool::new(true);
        let err = compute_solution_bounded(&seed_of(0x99), 32, MAX_POW_ITERATIONS, None, &cancel)
            .expect_err("must give up when cancelled");
        assert!(err.contains("cancelled"), "unexpected error: {err}");
    }

    #[test]
    fn challenge_seed_bytes_round_trips_and_rejects_junk() {
        let good = Challenge {
            address: "0x0000000000000000000000000000000000000001".into(),
            seed: hex::encode(seed_of(0xAA)),
            difficulty_bits: 12,
            expires_at: 0,
        };
        assert_eq!(good.seed_bytes().unwrap(), seed_of(0xAA));

        let short = Challenge {
            seed: "aabb".into(),
            ..good.clone()
        };
        assert!(short.seed_bytes().unwrap_err().contains("expected 32"));

        let junk = Challenge {
            seed: "zz".repeat(32),
            ..good
        };
        assert!(junk.seed_bytes().unwrap_err().contains("not hex"));
    }
}
