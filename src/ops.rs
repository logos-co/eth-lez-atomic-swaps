//! Durable, privacy-safe operations ledger for public-trial swap outcomes
//! (issue #98).
//!
//! ## What this is for
//!
//! PR #94 shipped the *qualitative* half of "how many people tried this": a
//! Basecamp-compatible feedback form (`.github/ISSUE_TEMPLATE/trial-feedback.yml`)
//! that public testers fill in by hand, voluntarily. Its count is one `gh`
//! query away, but it is a self-selected sample — most testers never file
//! anything either way.
//!
//! This module is the *quantitative* half: it durably counts distinct trial
//! swaps that the maker's standing liquidity bot (`swap-cli maker --loop`)
//! actually processed, whether or not the taker ever opens GitHub. It adds
//! **zero outbound calls** — the ledger is a local, append-only file the
//! operator already controls, exactly like the existing crash-recovery
//! journal ([`crate::cli::bot::StateStore`]) it sits next to.
//!
//! ## Relationship to the crash-recovery journal
//!
//! [`crate::cli::bot::StateStore`] / [`crate::swap::maker::SwapJournal`]
//! exist purely for **fund safety**: they track only *currently in-flight*
//! swaps and forget an entry the moment it reaches a confirmed terminal
//! state (so a restart never re-locks a resolved escrow). That means the
//! fund-safety journal is structurally unable to answer "how many swaps have
//! we EVER served" — completed/refunded history simply disappears.
//!
//! This ledger is the durable, append-only complement: one `Accepted` event
//! when the fund-safety journal accepts a swap (the same moment, always
//! recorded second so an ops-ledger write can NEVER gate a lock decision),
//! and exactly one terminal event once the swap is confirmed resolved — by
//! whichever process observes the resolution, including a restart's
//! `reconcile()` pass.
//!
//! ## Why hashlock+swap_id, not `acceptance_id`
//!
//! Issue #98 frames this around the protocol's opaque `acceptance_id` from a
//! future **signed** offer/acceptance handshake. That handshake does not
//! exist yet in this repo (`grep -r acceptance_id src` finds nothing) — v1
//! offers are unsigned, exactly as the issue's own "Release relationship"
//! section anticipates ("unsigned v1 offers do not provide the stable
//! acceptance identity needed for trustworthy deduplication"). Until that
//! lands, this ledger uses the identifiers that already ARE stable and
//! protocol-native today: the **hashlock** (the LEZ HTLC program derives
//! exactly one escrow PDA per hashlock, so it is already the de facto
//! one-swap-per-hashlock identity — see `classify_candidate`'s doc in
//! `swap::maker`) and the EthHTLC **swap id**. When signed acceptances land,
//! swap the dedupe key for `acceptance_id` here; the record shape and CLI
//! surface do not need to change.
//!
//! ## Privacy boundary
//!
//! [`OpsRecord`] cannot represent a wallet/counterparty address, a peer id,
//! an IP, a topic, a preimage, a private key, or unbounded/raw error text —
//! there is no field for any of them. Failures are the bounded
//! [`FailureCode`] enum, never a `String`. See `mod tests` for a fixture
//! that proves a forged/malformed ledger line is dropped rather than
//! silently accepted.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::{Result, SwapError};
use crate::swap::refund::now_unix;

/// Ledger schema version, mirroring the `trial-evidence/1` pattern from PR
/// #94 so both allowlists can be versioned independently.
pub const SCHEMA_VERSION: &str = "trial-ops/1";

/// Default ops-ledger path: sibling of the crash-recovery state file, named
/// `maker-ops.jsonl`. Mirrors `cli::bot::default_status_file`'s convention.
/// `--ops-file` (`MAKER_OPS_FILE`) overrides it.
pub fn default_ledger_file(state_file: &Path) -> PathBuf {
    match state_file.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("maker-ops.jsonl"),
        _ => PathBuf::from("maker-ops.jsonl"),
    }
}

/// Bounded, versioned failure taxonomy. Deliberately NOT `String`: unbounded
/// error text is exactly the kind of field that can carry a secret, an RPC
/// URL, or a path by accident, which is the privacy boundary issue #98 draws.
/// Add a new variant when a genuinely new confirmed-unresolvable termination
/// is identified; never turn this into free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    /// The maker's ETH claim could not be confirmed after exhausting retries,
    /// including across a restart's `reconcile()` pass.
    EthClaimUnresolved,
    /// The LEZ refund could not be confirmed after exhausting retries.
    LezRefundUnresolved,
    /// The lock committed on-chain but its funding transfer never landed —
    /// permanently unrecoverable (see `StateStore::quarantine`,
    /// `is_partial_lock_wedge`).
    PartialLockWedge,
    /// A journaled pre-lock entry with no corresponding on-chain escrow ever
    /// observed — the LEZ lock most plausibly failed before it committed.
    PreLockAbandoned,
    /// The journal entry itself could not be parsed/recovered.
    CorruptEntry,
    /// A bounded catch-all for any other confirmed-unresolvable termination.
    Other,
}

/// Terminal (or not-yet-terminal) lifecycle state of one ledger record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RecordState {
    /// A signed... today, an on-chain-matched... acceptance reserved this
    /// maker offer; not yet resolved.
    Accepted,
    Completed,
    Refunded,
    Failed {
        code: FailureCode,
    },
}

/// One durable, versioned operation record — the materialized view of a
/// hashlock's event history. Every field is public/on-chain-derivable or a
/// bounded label; see the module-level privacy boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpsRecord {
    pub version: String,
    /// 64-char lowercase hex hashlock — the dedupe key (see module docs).
    pub hashlock: String,
    /// 0x-prefixed 32-byte hex EthHTLC swap id.
    pub swap_id: String,
    /// This maker binary's own release (`CARGO_PKG_VERSION` of the
    /// `swap-orchestrator` crate). Distinct from the swap/swap_ui module
    /// versions public testers install in Basecamp.
    pub release: String,
    pub accepted_at: u64,
    pub final_at: Option<u64>,
    pub state: RecordState,
}

/// Append-only wire events. `Terminal` carries only the dedupe key plus the
/// new state — replaying it against a HashMap is what makes the whole ledger
/// idempotent under duplicate/replayed/forged lines (see [`apply`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LedgerEvent {
    Accepted {
        hashlock: String,
        swap_id: String,
        release: String,
        ts: u64,
    },
    Terminal {
        hashlock: String,
        state: RecordState,
        ts: u64,
    },
}

/// A hashlock is a 32-byte value; a swap id likewise. Reject anything that
/// does not look like a plausible hex identifier of the right length before
/// it is ever written to the ledger — belt against a caller accidentally
/// passing free text (e.g. an error message) through the hashlock/swap_id
/// parameters.
fn looks_like_hex_id(s: &str, expected_bytes: usize) -> bool {
    let s = s.trim_start_matches("0x");
    s.len() == expected_bytes * 2 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Fold one wire event into the materialized record map. This is the ONLY
/// place state transitions happen, and it enforces every idempotency/
/// forgery-resistance invariant issue #98 asks for:
///
/// - a `Terminal` event for a hashlock with no prior `Accepted` record is
///   dropped — a forged/out-of-order terminal notification can never
///   fabricate an `accepted` count (acceptance test: "malformed, forged,
///   untrusted, or pre-reservation messages never increment `accepted`").
/// - a duplicate `Accepted` for an already-known hashlock is a no-op.
/// - a duplicate/replayed `Terminal` once a record is already terminal is a
///   no-op — the first confirmed terminal state wins and can never be
///   overwritten by a later (e.g. re-delivered) event.
fn apply(records: &mut HashMap<String, OpsRecord>, event: LedgerEvent) {
    match event {
        LedgerEvent::Accepted {
            hashlock,
            swap_id,
            release,
            ts,
        } => {
            records.entry(hashlock.clone()).or_insert(OpsRecord {
                version: SCHEMA_VERSION.to_string(),
                hashlock,
                swap_id,
                release,
                accepted_at: ts,
                final_at: None,
                state: RecordState::Accepted,
            });
        }
        LedgerEvent::Terminal {
            hashlock,
            state,
            ts,
        } => {
            if let Some(r) = records.get_mut(&hashlock)
                && matches!(r.state, RecordState::Accepted)
            {
                r.state = state;
                r.final_at = Some(ts);
            }
            // Unknown hashlock, or already terminal: no-op by design.
        }
    }
}

/// Unique counts by outcome, grouped exactly as issue #98 asks: "unique
/// counts grouped by `accepted`, `completed`, `refunded`, and `failed`."
/// `accepted_total` counts every distinct hashlock the ledger has ever seen
/// (whatever its current state) — the true attempt denominator. `in_flight`
/// is the subset still unresolved (accepted but no terminal state yet,
/// including one that survived a maker restart mid-swap).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpsReport {
    pub accepted_total: usize,
    pub completed: usize,
    pub refunded: usize,
    pub failed: usize,
    pub failed_by_code: Vec<(FailureCode, usize)>,
    pub in_flight: usize,
}

/// The trait `swap::maker` depends on (mirroring [`crate::swap::maker::SwapJournal`]'s
/// dependency-inversion shape, so the swap layer never depends on the CLI
/// layer). A recorder failure is ALWAYS best-effort and must never affect
/// swap safety or availability — see [`OpsLedger`]'s doc for why.
pub trait OpsRecorder: Send + Sync {
    /// Record that a signed... today, an on-chain-matched... acceptance
    /// reserved this maker offer. Idempotent on `hashlock_hex`.
    fn accepted(&self, hashlock_hex: &str, swap_id: &str);
    /// Record a confirmed `completed` terminal state. Idempotent.
    fn completed(&self, hashlock_hex: &str);
    /// Record a confirmed `refunded` terminal state. Idempotent.
    fn refunded(&self, hashlock_hex: &str);
    /// Record a confirmed-unresolvable `failed` terminal state. Idempotent.
    fn failed(&self, hashlock_hex: &str, code: FailureCode);
}

/// JSONL-backed operations ledger: one durable line per event, replayed into
/// an in-memory map on load. Deliberately append-only rather than
/// rewrite-in-place like [`crate::cli::bot::StateStore`] — nothing in this
/// file is ever mutated or deleted, only appended, which is what makes "durable,
/// versioned operation record" (issue #98) and simple auditing
/// (`wc -l`, `jq`) both true at once.
///
/// Every write here is **best-effort**: a lost write costs one under-counted
/// row, never a swap. The swap-safety decision (whether to lock LEZ) is
/// already gated by [`crate::cli::bot::StateStore`] before this is ever
/// consulted, and this ledger's own writes happen strictly AFTER that gate,
/// so an ops-ledger I/O failure can never block or corrupt a swap in
/// progress — only under-report it, which the next reconcile's terminal call
/// (itself idempotent) can still land later.
pub struct OpsLedger {
    path: PathBuf,
    release: String,
    records: Mutex<HashMap<String, OpsRecord>>,
}

impl OpsLedger {
    /// Load (or create empty) the ledger at `path`. A corrupt/unparseable
    /// line is skipped with a warning rather than failing startup — this is
    /// a reporting artifact, not the fund-safety journal.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut records = HashMap::new();
        match std::fs::read_to_string(&path) {
            Ok(raw) => {
                for (i, line) in raw.lines().enumerate() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<LedgerEvent>(line) {
                        Ok(event) => apply(&mut records, event),
                        Err(e) => warn!(
                            "ops ledger: skipping unparseable line {} in {}: {e}",
                            i + 1,
                            path.display()
                        ),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(SwapError::InvalidConfig(format!(
                    "cannot read ops ledger {}: {e}",
                    path.display()
                )));
            }
        }
        Ok(Self {
            path,
            release: env!("CARGO_PKG_VERSION").to_string(),
            records: Mutex::new(records),
        })
    }

    fn append(&self, event: &LedgerEvent) -> std::io::Result<()> {
        let line = serde_json::to_string(event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(f, "{line}")?;
        f.sync_all()?;
        Ok(())
    }

    fn record_terminal(&self, hashlock_hex: &str, state: RecordState) {
        if !looks_like_hex_id(hashlock_hex, 32) {
            warn!(
                "ops ledger: refusing non-hashlock-shaped key '{hashlock_hex}' for terminal state"
            );
            return;
        }
        let mut records = self.records.lock().expect("ops ledger lock");
        // Idempotent: only a currently-`Accepted` record may transition.
        match records.get(hashlock_hex) {
            Some(r) if matches!(r.state, RecordState::Accepted) => {}
            _ => return,
        }
        let ts = now_unix();
        let event = LedgerEvent::Terminal {
            hashlock: hashlock_hex.to_string(),
            state,
            ts,
        };
        if let Err(e) = self.append(&event) {
            warn!("ops ledger: failed to durably record terminal state for {hashlock_hex}: {e}");
            return;
        }
        apply(&mut records, event);
    }

    /// Unique counts by outcome (issue #98's operator report).
    pub fn report(&self) -> OpsReport {
        let records = self.records.lock().expect("ops ledger lock");
        let mut completed = 0usize;
        let mut refunded = 0usize;
        let mut failed = 0usize;
        let mut failed_by_code: HashMap<FailureCode, usize> = HashMap::new();
        for r in records.values() {
            match &r.state {
                RecordState::Accepted => {}
                RecordState::Completed => completed += 1,
                RecordState::Refunded => refunded += 1,
                RecordState::Failed { code } => {
                    failed += 1;
                    *failed_by_code.entry(*code).or_insert(0) += 1;
                }
            }
        }
        let accepted_total = records.len();
        let mut failed_by_code: Vec<(FailureCode, usize)> = failed_by_code.into_iter().collect();
        failed_by_code.sort();
        OpsReport {
            accepted_total,
            completed,
            refunded,
            failed,
            failed_by_code,
            in_flight: accepted_total.saturating_sub(completed + refunded + failed),
        }
    }

    /// Snapshot of every record, for `--ops-report --json` detail / debugging.
    pub fn records_snapshot(&self) -> Vec<OpsRecord> {
        let mut v: Vec<OpsRecord> = self
            .records
            .lock()
            .expect("ops ledger lock")
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.accepted_at.cmp(&b.accepted_at));
        v
    }
}

impl OpsRecorder for OpsLedger {
    fn accepted(&self, hashlock_hex: &str, swap_id: &str) {
        if !looks_like_hex_id(hashlock_hex, 32) || !looks_like_hex_id(swap_id, 32) {
            warn!(
                "ops ledger: refusing non-hex-shaped accepted record (hashlock={hashlock_hex}, \
                 swap_id={swap_id})"
            );
            return;
        }
        let mut records = self.records.lock().expect("ops ledger lock");
        if records.contains_key(hashlock_hex) {
            return; // idempotent: already accepted (or further along)
        }
        let ts = now_unix();
        let event = LedgerEvent::Accepted {
            hashlock: hashlock_hex.to_string(),
            swap_id: swap_id.to_string(),
            release: self.release.clone(),
            ts,
        };
        if let Err(e) = self.append(&event) {
            warn!("ops ledger: failed to durably record accepted {hashlock_hex}: {e}");
            return;
        }
        apply(&mut records, event);
    }

    fn completed(&self, hashlock_hex: &str) {
        self.record_terminal(hashlock_hex, RecordState::Completed);
    }

    fn refunded(&self, hashlock_hex: &str) {
        self.record_terminal(hashlock_hex, RecordState::Refunded);
    }

    fn failed(&self, hashlock_hex: &str, code: FailureCode) {
        self.record_terminal(hashlock_hex, RecordState::Failed { code });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "swap-ops-ledger-test-{name}-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn hex_id(byte: u8) -> String {
        hex::encode([byte; 32])
    }

    // One accepted + completed swap produces accepted=1, completed=1.
    #[test]
    fn accepted_then_completed_counts_one_each() {
        let path = tmp_path("basic");
        let ledger = OpsLedger::load(&path).unwrap();
        let hl = hex_id(0x01);
        ledger.accepted(&hl, &hex_id(0xAA));
        ledger.completed(&hl);

        let report = ledger.report();
        assert_eq!(report.accepted_total, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(report.refunded, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.in_flight, 0);
        std::fs::remove_file(&path).ok();
    }

    // A replayed acceptance and duplicate terminal notification keep counts at one.
    #[test]
    fn replayed_events_are_idempotent() {
        let path = tmp_path("idempotent");
        let ledger = OpsLedger::load(&path).unwrap();
        let hl = hex_id(0x02);
        let swap_id = hex_id(0xBB);

        ledger.accepted(&hl, &swap_id);
        ledger.accepted(&hl, &swap_id); // replay
        ledger.accepted(&hl, &swap_id); // replay again

        ledger.completed(&hl);
        ledger.completed(&hl); // duplicate terminal
        ledger.refunded(&hl); // even a DIFFERENT terminal after the first must not flip it

        let report = ledger.report();
        assert_eq!(
            report.accepted_total, 1,
            "replayed acceptance must not double-count"
        );
        assert_eq!(report.completed, 1);
        assert_eq!(
            report.refunded, 0,
            "first confirmed terminal state wins, permanently"
        );
        std::fs::remove_file(&path).ok();
    }

    // Accepted-then-refunded and accepted-then-failed are counted distinctly.
    #[test]
    fn refunded_and_failed_are_distinct_buckets() {
        let path = tmp_path("distinct");
        let ledger = OpsLedger::load(&path).unwrap();
        let hl_refunded = hex_id(0x03);
        let hl_failed = hex_id(0x04);

        ledger.accepted(&hl_refunded, &hex_id(0xC1));
        ledger.refunded(&hl_refunded);

        ledger.accepted(&hl_failed, &hex_id(0xC2));
        ledger.failed(&hl_failed, FailureCode::EthClaimUnresolved);

        let report = ledger.report();
        assert_eq!(report.accepted_total, 2);
        assert_eq!(report.refunded, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(
            report.failed_by_code,
            vec![(FailureCode::EthClaimUnresolved, 1)]
        );
        std::fs::remove_file(&path).ok();
    }

    // Restart after acceptance and before terminal state preserves one
    // in-flight trial, and later records exactly one terminal result — the
    // core "survives a maker restart" acceptance test, simulated by
    // dropping the ledger and reloading from the same file (what actually
    // happens across a process restart).
    #[test]
    fn restart_preserves_in_flight_then_terminalizes_once() {
        let path = tmp_path("restart");
        {
            let ledger = OpsLedger::load(&path).unwrap();
            ledger.accepted(&hex_id(0x05), &hex_id(0xD1));
        } // ledger dropped — simulates process exit

        // Reload (simulates process restart) — the swap is still in-flight.
        let reloaded = OpsLedger::load(&path).unwrap();
        let report = reloaded.report();
        assert_eq!(report.accepted_total, 1);
        assert_eq!(report.in_flight, 1);
        assert_eq!(report.completed, 0);

        // The restarted process's reconcile pass resolves it.
        reloaded.completed(&hex_id(0x05));
        let report = reloaded.report();
        assert_eq!(report.completed, 1);
        assert_eq!(report.in_flight, 0);

        // A second restart must not re-count or flip the terminal state.
        let reloaded_again = OpsLedger::load(&path).unwrap();
        let report = reloaded_again.report();
        assert_eq!(report.accepted_total, 1);
        assert_eq!(report.completed, 1);
        std::fs::remove_file(&path).ok();
    }

    // Malformed, forged, untrusted, or pre-reservation messages never
    // increment `accepted`: a Terminal event with no prior Accepted line is
    // silently dropped, never fabricating a record.
    #[test]
    fn terminal_without_prior_acceptance_is_dropped() {
        let path = tmp_path("forged");
        let ledger = OpsLedger::load(&path).unwrap();
        // A forged/out-of-order terminal event for a hashlock that was NEVER
        // accepted must not create a phantom accepted+completed record.
        ledger.completed(&hex_id(0x06));
        ledger.refunded(&hex_id(0x07));
        ledger.failed(&hex_id(0x08), FailureCode::Other);

        let report = ledger.report();
        assert_eq!(
            report.accepted_total, 0,
            "a bare terminal event must never fabricate an accepted record"
        );
        std::fs::remove_file(&path).ok();
    }

    // A fixture containing secrets/addresses/raw errors is rejected before
    // persistence: the recorder validates hashlock/swap_id SHAPE (64-hex),
    // so a caller that accidentally passes free text (an address, a raw
    // error message, a preimage, a URL) never gets it written to the durable
    // ledger at all.
    #[test]
    fn non_hex_shaped_input_is_rejected_not_persisted() {
        let path = tmp_path("shape-guard");
        let ledger = OpsLedger::load(&path).unwrap();

        // Things that must never end up in the ledger if ever passed by
        // mistake: an ETH address (20 bytes, wrong length for a hashlock), a
        // raw error string, and a URL with embedded credentials.
        let poison_inputs = [
            "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", // an address
            "maker: ETH claim attempt 3/6 failed: connection refused",
            "wss://user:secret-token@rpc.example.com/v1/abcdef",
            "not-hex-at-all",
        ];
        for poison in poison_inputs {
            ledger.accepted(poison, &hex_id(0xEE));
            ledger.completed(poison);
        }

        assert_eq!(
            ledger.report().accepted_total,
            0,
            "non-hashlock-shaped input must never be persisted into the ledger"
        );
        std::fs::remove_file(&path).ok();
    }

    // A corrupt/unparseable JSONL line (e.g. truncated by a crash mid-write,
    // or hand-edited) is skipped on load rather than poisoning the whole
    // ledger or crashing the maker — this is a reporting artifact, not the
    // fund-safety journal.
    #[test]
    fn corrupt_line_is_skipped_on_load() {
        let path = tmp_path("corrupt");
        let good = serde_json::to_string(&LedgerEvent::Accepted {
            hashlock: hex_id(0x09),
            swap_id: hex_id(0xF0),
            release: "0.1.0".into(),
            ts: 1,
        })
        .unwrap();
        std::fs::write(&path, format!("{good}\nnot valid json at all\n")).unwrap();

        let ledger = OpsLedger::load(&path).unwrap();
        let report = ledger.report();
        assert_eq!(
            report.accepted_total, 1,
            "the one valid line must still load"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn looks_like_hex_id_validates_length_and_charset() {
        assert!(looks_like_hex_id(&"ab".repeat(32), 32));
        assert!(looks_like_hex_id(&format!("0x{}", "ab".repeat(32)), 32));
        assert!(!looks_like_hex_id(&"ab".repeat(20), 32)); // wrong length (an address)
        assert!(!looks_like_hex_id("not hex", 32));
    }
}
