//! Outstanding PoW challenges, keyed by the address they were issued for.
//!
//! Server-side rather than a signed stateless token, for two reasons:
//! - `POST /drip` then needs only `{address, pow_solution}` — the client never
//!   has to echo back a blob it does not understand, and a curl demo is one
//!   readable line.
//! - Consumption is trivially exactly-once: the entry is *removed* when a
//!   solution is accepted, so a replayed solution finds nothing to spend.
//!
//! In memory only. A restart drops outstanding challenges, which costs an
//! in-flight user one re-solve — unlike dropping the *ledger*, which would
//! reset real cooldowns and so is journalled to disk.

use std::collections::HashMap;
use std::time::Duration;

use eth_faucet_pow::Challenge;
use rand::RngCore as _;

/// Why a submitted solution was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeError {
    /// No outstanding challenge for this address: never asked for one, already
    /// spent it, or the service restarted.
    Missing,
    Expired,
    WrongSolution,
}

impl ChallengeError {
    pub fn code(&self) -> &'static str {
        match self {
            ChallengeError::Missing => "no_challenge",
            ChallengeError::Expired => "challenge_expired",
            ChallengeError::WrongSolution => "bad_solution",
        }
    }

    pub fn message(&self) -> String {
        match self {
            ChallengeError::Missing => {
                "No puzzle is outstanding for this address — ask for a new one and try again."
                    .to_string()
            }
            ChallengeError::Expired => {
                "That puzzle expired before the answer arrived — ask for a new one and try again."
                    .to_string()
            }
            ChallengeError::WrongSolution => "That answer does not solve the puzzle.".to_string(),
        }
    }
}

#[derive(Default)]
pub struct Challenges {
    /// One outstanding challenge per address. Re-issuing REPLACES the previous
    /// one rather than accumulating: otherwise an attacker could bank a pile of
    /// pre-solved challenges for one address and spend them the moment its
    /// cooldown lapsed.
    outstanding: HashMap<String, Challenge>,
}

impl Challenges {
    /// Issue (and store) a fresh challenge for `address`.
    ///
    /// The seed comes from the OS CSPRNG, so a client cannot predict the next
    /// one and start solving before it asks.
    pub fn issue(
        &mut self,
        address: &str,
        difficulty_bits: u8,
        ttl: Duration,
        now: u64,
    ) -> Challenge {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);

        let challenge = Challenge {
            address: address.to_string(),
            seed: hex::encode(seed),
            difficulty_bits,
            expires_at: now.saturating_add(ttl.as_secs()),
        };
        self.outstanding
            .insert(address.to_string(), challenge.clone());
        challenge
    }

    /// Verify `solution` against the outstanding challenge for `address` and,
    /// on success, CONSUME it. A wrong answer leaves the challenge in place so
    /// an honest client whose first attempt raced the expiry can retry; a
    /// correct one is spent, so it can never be replayed.
    pub fn redeem(
        &mut self,
        address: &str,
        solution: u128,
        now: u64,
    ) -> Result<Challenge, ChallengeError> {
        let challenge = self
            .outstanding
            .get(address)
            .ok_or(ChallengeError::Missing)?
            .clone();

        if now > challenge.expires_at {
            self.outstanding.remove(address);
            return Err(ChallengeError::Expired);
        }

        // A seed this service issued is always 32 bytes of hex; a decode
        // failure here would be memory corruption, not user input. Treat it
        // as a wrong answer rather than panicking in a request handler.
        let seed = challenge
            .seed_bytes()
            .map_err(|_| ChallengeError::WrongSolution)?;
        if !eth_faucet_pow::validate_solution(&seed, challenge.difficulty_bits, solution) {
            return Err(ChallengeError::WrongSolution);
        }

        self.outstanding.remove(address);
        Ok(challenge)
    }

    /// Drop expired entries. Called opportunistically on every issue, so an
    /// idle faucet does not need a sweeper task and a busy one cannot grow a
    /// map of dead challenges.
    pub fn prune(&mut self, now: u64) {
        self.outstanding.retain(|_, c| now <= c.expires_at);
    }

    pub fn outstanding_count(&self) -> usize {
        self.outstanding.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    const ADDR: &str = "0x000000000000000000000000000000000000dead";
    const OTHER: &str = "0x000000000000000000000000000000000000beef";
    const TTL: Duration = Duration::from_secs(300);

    fn solve(challenge: &Challenge) -> u128 {
        let cancel = AtomicBool::new(false);
        eth_faucet_pow::compute_solution_bounded(
            &challenge.seed_bytes().unwrap(),
            challenge.difficulty_bits,
            eth_faucet_pow::MAX_POW_ITERATIONS,
            None,
            &cancel,
        )
        .unwrap()
    }

    #[test]
    fn a_solved_challenge_is_accepted_once_and_then_gone() {
        let mut challenges = Challenges::default();
        let now = 1_000_000;
        let challenge = challenges.issue(ADDR, 10, TTL, now);
        let solution = solve(&challenge);

        assert_eq!(challenges.redeem(ADDR, solution, now).unwrap(), challenge);
        // Replay finds nothing to spend.
        assert_eq!(
            challenges.redeem(ADDR, solution, now),
            Err(ChallengeError::Missing)
        );
    }

    #[test]
    fn a_wrong_answer_is_refused_but_keeps_the_challenge_claimable() {
        let mut challenges = Challenges::default();
        let now = 1_000_000;
        let challenge = challenges.issue(ADDR, 10, TTL, now);
        let solution = solve(&challenge);

        assert_eq!(
            challenges.redeem(ADDR, solution.wrapping_add(1), now),
            Err(ChallengeError::WrongSolution)
        );
        // The honest client's real answer still works.
        assert!(challenges.redeem(ADDR, solution, now).is_ok());
    }

    #[test]
    fn a_solution_earned_for_one_address_cannot_be_spent_on_another() {
        let mut challenges = Challenges::default();
        let now = 1_000_000;
        let mine = challenges.issue(ADDR, 10, TTL, now);
        challenges.issue(OTHER, 10, TTL, now);
        let solution = solve(&mine);

        // Different address -> different seed -> the answer does not verify.
        assert_eq!(
            challenges.redeem(OTHER, solution, now),
            Err(ChallengeError::WrongSolution)
        );
    }

    #[test]
    fn an_expired_challenge_is_refused_and_cleared() {
        let mut challenges = Challenges::default();
        let now = 1_000_000;
        let challenge = challenges.issue(ADDR, 10, TTL, now);
        let solution = solve(&challenge);

        let past_expiry = challenge.expires_at + 1;
        assert_eq!(
            challenges.redeem(ADDR, solution, past_expiry),
            Err(ChallengeError::Expired)
        );
        assert_eq!(
            challenges.redeem(ADDR, solution, past_expiry),
            Err(ChallengeError::Missing)
        );
    }

    #[test]
    fn a_challenge_is_still_claimable_at_the_instant_it_expires() {
        let mut challenges = Challenges::default();
        let now = 1_000_000;
        let challenge = challenges.issue(ADDR, 10, TTL, now);
        let solution = solve(&challenge);
        assert!(challenges.redeem(ADDR, solution, challenge.expires_at).is_ok());
    }

    #[test]
    fn re_issuing_replaces_rather_than_accumulates() {
        // Otherwise an attacker banks pre-solved challenges for one address
        // and spends them the moment its cooldown lapses.
        let mut challenges = Challenges::default();
        let now = 1_000_000;
        let first = challenges.issue(ADDR, 10, TTL, now);
        let second = challenges.issue(ADDR, 10, TTL, now);
        assert_ne!(first.seed, second.seed, "each issue draws a fresh seed");
        assert_eq!(challenges.outstanding_count(), 1);

        assert_eq!(
            challenges.redeem(ADDR, solve(&first), now),
            Err(ChallengeError::WrongSolution)
        );
        assert!(challenges.redeem(ADDR, solve(&second), now).is_ok());
    }

    #[test]
    fn pruning_drops_only_expired_entries() {
        let mut challenges = Challenges::default();
        let now = 1_000_000;
        challenges.issue(ADDR, 10, Duration::from_secs(10), now);
        challenges.issue(OTHER, 10, Duration::from_secs(1000), now);

        challenges.prune(now + 100);
        assert_eq!(challenges.outstanding_count(), 1);
        assert_eq!(
            challenges.redeem(ADDR, 0, now + 100),
            Err(ChallengeError::Missing)
        );
    }
}
