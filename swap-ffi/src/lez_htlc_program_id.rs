//! Checked-in canonical LEZ HTLC program ID surfaced to catalog users.
//!
//! The value is the deterministic risc0 ImageID of the `lez-htlc` guest
//! (`programs/lez-htlc/methods`), hex-encoded the same way the swap
//! orchestrator encodes it on-chain: each of the 8 `u32` words of
//! `lez_htlc_methods::LEZ_HTLC_PROGRAM_ID` serialized little-endian, then
//! hex-encoded (64 lowercase hex chars, no `0x` prefix).
//!
//! Why checked in instead of computed at build time: embedding the ID via the
//! `lez_htlc_methods` crate requires the risc0 toolchain and a nested guest
//! cargo build, which the sandboxed Nix module build cannot run. The ImageID
//! is deterministic for a given guest + LEZ pin, so a constant is safe — but
//! it MUST match the program actually DEPLOYED on the target LEZ network,
//! because that is what a swap on that network executes against.
//!
//! Updating (one-liner): change the constant below after (re)deploying the
//! guest. The `demo`-gated `checked_in_program_id_matches_guest_image_id`
//! test (`cargo test -p swap-ffi --features demo`) is the drift tripwire:
//! it asserts this constant equals the pinned guest's ImageID.

/// Canonical LEZ HTLC program ID: the guest ImageID under the LEZ v0.2.2 pin
/// (tag `v0.2.2`, `d6e4ae69`), which this branch builds — the checked-in
/// constant and the pinned guest's ImageID converge, and the drift test above
/// enforces it. Verified locally 2026-08-06 via
/// `cargo test -p swap-ffi --features demo`.
///
/// The public testnet was reset + upgraded to v0.2.2 on 2026-08-06; the HTLC
/// program is (re)deployed against this ImageID as part of that rollout (the
/// deploy tx / on-chain redeploy is handled by the release orchestrator, out
/// of band from this build-only change).
pub(crate) const LEZ_HTLC_PROGRAM_ID_HEX: &str =
    "9eb88f51aae87a58fb74b8d2dc7327b39333585e63280e3f9cf8d86dac0ed702";
