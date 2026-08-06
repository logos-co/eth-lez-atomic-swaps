//! Version-skew fingerprint: compare the sequencer's builtin program ImageIDs
//! (`getProgramIds` RPC) against the ImageIDs embedded in this binary at build
//! time by the v0.2.2-pinned LEZ crates.
//!
//! This is the check that catches silent version skew: a client pinned to the
//! wrong LEZ version does not error — the sequencer silently drops
//! transactions whose `program_id` it does not recognize. On mismatch the
//! server keeps read tools live but hard-refuses writes.

use std::collections::BTreeMap;

use lee_core::program::ProgramId;
use sequencer_service_rpc::{RpcClient as _, SequencerClient};
use serde::Serialize;

/// The LEZ tag this binary embeds.
pub const CLIENT_TAG: &str = "v0.2.2";
/// Commit of the v0.2.2 tag.
pub const CLIENT_COMMIT: &str = "d6e4ae69";

/// Redact a sequencer URL down to `scheme://host[:port]` for anything that is
/// logged or serialized into a tool result. Strips userinfo (basic-auth
/// credentials), path, and query — which may carry access tokens — while
/// keeping enough to identify the endpoint. The raw URL is only ever used to
/// open the connection, never surfaced.
pub fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(u) => {
            let scheme = u.scheme();
            match (u.host_str(), u.port()) {
                (Some(host), Some(port)) => format!("{scheme}://{host}:{port}"),
                (Some(host), None) => format!("{scheme}://{host}"),
                (None, _) => format!("{scheme}://<redacted>"),
            }
        }
        Err(_) => "<unparseable-url>".to_string(),
    }
}

/// Canonical hex rendering of a ProgramId: each u32 word little-endian,
/// concatenated, hex-encoded (64 lowercase chars, no 0x) — same scheme as
/// `swap-ffi/src/lez_htlc_program_id.rs` and `parse_program_id`.
pub fn program_id_hex(id: &ProgramId) -> String {
    let bytes: Vec<u8> = id.iter().flat_map(|w| w.to_le_bytes()).collect();
    hex::encode(bytes)
}

/// The five builtin ImageIDs embedded at build time from the pinned LEZ
/// crates. Names match the sequencer's `getProgramIds` response exactly.
pub fn embedded_program_ids() -> BTreeMap<String, ProgramId> {
    BTreeMap::from([
        ("amm".to_string(), programs::amm().id()),
        (
            "authenticated_transfer".to_string(),
            programs::authenticated_transfer().id(),
        ),
        ("pinata".to_string(), programs::pinata().id()),
        (
            "privacy_preserving_circuit".to_string(),
            lee::PRIVACY_PRESERVING_CIRCUIT_ID,
        ),
        ("token".to_string(), programs::token().id()),
    ])
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgramFingerprint {
    /// ImageID embedded in this binary (hex).
    pub embedded: String,
    /// ImageID reported by the sequencer (hex), if present.
    pub sequencer: Option<String>,
    pub matches: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FingerprintReport {
    pub sequencer_url: String,
    pub client_tag: &'static str,
    pub client_commit: &'static str,
    /// True iff every embedded builtin is present on the sequencer with a
    /// bit-for-bit identical ImageID.
    pub matched: bool,
    pub programs: BTreeMap<String, ProgramFingerprint>,
    /// Builtins the sequencer reports that this binary does not embed.
    pub extra_sequencer_programs: Vec<String>,
    /// Names of embedded builtins that mismatch or are missing remotely.
    pub mismatches: Vec<String>,
}

/// Pure comparison — separated from the RPC call for unit testing.
pub fn build_report(
    sequencer_url: &str,
    embedded: &BTreeMap<String, ProgramId>,
    remote: &BTreeMap<String, ProgramId>,
) -> FingerprintReport {
    let mut programs_out = BTreeMap::new();
    let mut mismatches = Vec::new();

    for (name, local_id) in embedded {
        let remote_id = remote.get(name);
        let matches = remote_id == Some(local_id);
        if !matches {
            mismatches.push(name.clone());
        }
        programs_out.insert(
            name.clone(),
            ProgramFingerprint {
                embedded: program_id_hex(local_id),
                sequencer: remote_id.map(program_id_hex),
                matches,
            },
        );
    }

    let extra_sequencer_programs: Vec<String> = remote
        .keys()
        .filter(|name| !embedded.contains_key(*name))
        .cloned()
        .collect();

    FingerprintReport {
        // Redacted: this field is serialized into tool results and logs.
        sequencer_url: redact_url(sequencer_url),
        client_tag: CLIENT_TAG,
        client_commit: CLIENT_COMMIT,
        matched: mismatches.is_empty(),
        programs: programs_out,
        extra_sequencer_programs,
        mismatches,
    }
}

/// Fetch the sequencer's builtin program ids and compare against the embedded
/// table. Errors are returned as strings (never panics) so a down sequencer
/// yields "unverified" rather than a crash.
pub async fn run_fingerprint(
    client: &SequencerClient,
    sequencer_url: &str,
) -> Result<FingerprintReport, String> {
    let remote = client
        .get_program_ids()
        .await
        .map_err(|e| format!("getProgramIds RPC failed: {e}"))?;
    Ok(build_report(sequencer_url, &embedded_program_ids(), &remote))
}

#[cfg(test)]
mod tests {
    use super::*;
    use swap_orchestrator::config::parse_program_id;

    #[test]
    fn redact_url_strips_credentials_path_and_query() {
        // Basic-auth userinfo and query token must not survive.
        assert_eq!(
            redact_url("https://user:s3cret@testnet.lez.logos.co/rpc?token=abc123"),
            "https://testnet.lez.logos.co"
        );
        assert_eq!(
            redact_url("http://localhost:3040/path"),
            "http://localhost:3040"
        );
        assert_eq!(redact_url("https://x"), "https://x");
        assert_eq!(redact_url("not a url"), "<unparseable-url>");
    }

    #[test]
    fn report_sequencer_url_is_redacted() {
        let embedded = embedded_program_ids();
        let report = build_report(
            "https://user:pw@seq.example.com:8443/v1?apikey=deadbeef",
            &embedded,
            &embedded.clone(),
        );
        assert_eq!(report.sequencer_url, "https://seq.example.com:8443");
        assert!(!report.sequencer_url.contains("pw"));
        assert!(!report.sequencer_url.contains("apikey"));
    }

    #[test]
    fn program_id_hex_round_trips_canonical_htlc_id() {
        let canonical = "9eb88f51aae87a58fb74b8d2dc7327b39333585e63280e3f9cf8d86dac0ed702";
        let id = parse_program_id(canonical).expect("parse");
        assert_eq!(program_id_hex(&id), canonical);
    }

    #[test]
    fn embedded_table_has_the_five_builtins() {
        let embedded = embedded_program_ids();
        let names: Vec<&str> = embedded.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            vec![
                "amm",
                "authenticated_transfer",
                "pinata",
                "privacy_preserving_circuit",
                "token",
            ]
        );
        // ImageIDs must be non-zero and pairwise distinct.
        let ids: Vec<_> = embedded.values().collect();
        for id in &ids {
            assert_ne!(**id, [0u32; 8]);
        }
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j]);
            }
        }
    }

    #[test]
    fn identical_maps_match() {
        let embedded = embedded_program_ids();
        let report = build_report("http://x", &embedded, &embedded.clone());
        assert!(report.matched);
        assert!(report.mismatches.is_empty());
        assert!(report.extra_sequencer_programs.is_empty());
        assert!(report.programs.values().all(|p| p.matches));
    }

    #[test]
    fn drifted_builtin_is_reported_with_structured_diff() {
        let embedded = embedded_program_ids();
        let mut remote = embedded.clone();
        let drifted = [0xDEADBEEFu32; 8];
        remote.insert("pinata".to_string(), drifted);
        remote.insert("brand_new_builtin".to_string(), [7u32; 8]);

        let report = build_report("http://x", &embedded, &remote);
        assert!(!report.matched);
        assert_eq!(report.mismatches, vec!["pinata".to_string()]);
        assert_eq!(
            report.extra_sequencer_programs,
            vec!["brand_new_builtin".to_string()]
        );
        let pinata = &report.programs["pinata"];
        assert!(!pinata.matches);
        assert_eq!(pinata.sequencer.as_deref(), Some(program_id_hex(&drifted).as_str()));
    }

    #[test]
    fn missing_remote_builtin_is_a_mismatch() {
        let embedded = embedded_program_ids();
        let mut remote = embedded.clone();
        remote.remove("token");
        let report = build_report("http://x", &embedded, &remote);
        assert!(!report.matched);
        assert_eq!(report.mismatches, vec!["token".to_string()]);
        assert_eq!(report.programs["token"].sequencer, None);
    }

    #[test]
    fn report_serializes_to_stable_json_shape() {
        let embedded = embedded_program_ids();
        let report = build_report("http://x", &embedded, &embedded.clone());
        let v = serde_json::to_value(&report).expect("serialize");
        assert_eq!(v["matched"], serde_json::json!(true));
        assert_eq!(v["client_tag"], serde_json::json!("v0.2.2"));
        assert!(v["programs"]["authenticated_transfer"]["embedded"].is_string());
        assert!(v["programs"]["amm"]["matches"].as_bool().unwrap());
    }
}
