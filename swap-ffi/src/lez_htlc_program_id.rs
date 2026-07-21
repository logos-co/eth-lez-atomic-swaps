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

/// Canonical LEZ HTLC program ID: the deployment on the public testnet
/// (`testnet.lez.logos.co`), deploy tx
/// `c1986c2af3fc007731533d958995507c8d8b1f447d5187cc1b8967ec238c7bf9`.
/// This is the guest ImageID under the LEZ v0.2.0 pin (tag `v0.2.0`,
/// a58fbce2), which this branch builds — the checked-in constant and the
/// pinned guest's ImageID converge, and the drift test above enforces it.
pub(crate) const LEZ_HTLC_PROGRAM_ID_HEX: &str =
    "27720b5b0345135d8e684eb172c27f5fb237548cc891a3ec889d0ed340504070";
