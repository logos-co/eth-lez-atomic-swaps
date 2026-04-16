//! Stub implementation of the `rln` crate.
//!
//! Why a stub instead of the real crate: upstream `rln 0.3.4` (transitive
//! dep of `waku-bindings`) pins many of its deps (`ark-serialize`,
//! `thiserror`, `color-eyre`, `wasmer`, …) to `=` exact versions that
//! conflict with our LEZ deps. Patching every `=` was a whack-a-mole
//! fight; an empty stub avoids the dep tree entirely.
//!
//! Why these particular symbols: `libwaku`'s Nim code statically
//! references the rln C-FFI surface (`new`, `flush`, `atomic_operation`,
//! `generate_rln_proof`, `verify_with_roots`, `poseidon_hash`, …) at
//! link time — even when RLN isn't enabled at runtime. Without these
//! `#[no_mangle] extern "C"` exports, the final binary fails to link
//! with `Undefined symbols for architecture arm64`.
//!
//! Each function returns failure (`false` / `0`) — this is fine because
//! we never enable `rln_relay` in `WakuNodeConfig`, so libwaku never
//! actually calls into RLN code at runtime. If you ever need to enable
//! RLN, swap this stub out for the real `rln` crate (and fight the dep
//! conflicts).
//!
//! See `delivery-dogfooding.md` for the full story.

#![allow(clippy::missing_safety_doc)]
#![allow(unused_variables)]

/// Opaque RLN context type. Never actually instantiated since none of
/// the stubbed functions allocate one.
#[repr(C)]
pub struct RLN {
    _opaque: [u8; 0],
}

/// FFI buffer struct — must match the layout the Nim caller expects.
#[repr(C)]
pub struct Buffer {
    pub ptr: *const u8,
    pub len: usize,
}

// ── RLN APIs ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn new(
    _tree_height: usize,
    _input_buffer: *const Buffer,
    _ctx: *mut *mut RLN,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn new_with_params(
    _tree_height: usize,
    _circom_buffer: *const Buffer,
    _zkey_buffer: *const Buffer,
    _vk_buffer: *const Buffer,
    _tree_config: *const Buffer,
    _ctx: *mut *mut RLN,
) -> bool {
    false
}

// ── Merkle tree APIs ────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn set_tree(_ctx: *mut RLN, _tree_height: usize) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn delete_leaf(_ctx: *mut RLN, _index: usize) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn set_leaf(_ctx: *mut RLN, _index: usize, _input_buffer: *const Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn get_leaf(_ctx: *mut RLN, _index: usize, _output_buffer: *mut Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn leaves_set(_ctx: *mut RLN) -> usize {
    0
}

#[no_mangle]
pub extern "C" fn set_next_leaf(_ctx: *mut RLN, _input_buffer: *const Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn set_leaves_from(
    _ctx: *mut RLN,
    _index: usize,
    _input_buffer: *const Buffer,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn init_tree_with_leaves(_ctx: *mut RLN, _input_buffer: *const Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn atomic_operation(
    _ctx: *mut RLN,
    _index: usize,
    _leaves_buffer: *const Buffer,
    _indices_buffer: *const Buffer,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn seq_atomic_operation(
    _ctx: *mut RLN,
    _leaves_buffer: *const Buffer,
    _indices_buffer: *const Buffer,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn get_root(_ctx: *const RLN, _output_buffer: *mut Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn get_proof(_ctx: *const RLN, _index: usize, _output_buffer: *mut Buffer) -> bool {
    false
}

// ── zkSNARK APIs ────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn prove(
    _ctx: *mut RLN,
    _input_buffer: *const Buffer,
    _output_buffer: *mut Buffer,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn verify(
    _ctx: *const RLN,
    _proof_buffer: *const Buffer,
    _proof_is_valid_ptr: *mut bool,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn generate_rln_proof(
    _ctx: *mut RLN,
    _input_buffer: *const Buffer,
    _output_buffer: *mut Buffer,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn verify_rln_proof(
    _ctx: *const RLN,
    _proof_buffer: *const Buffer,
    _proof_is_valid_ptr: *mut bool,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn verify_with_roots(
    _ctx: *const RLN,
    _proof_buffer: *const Buffer,
    _roots_buffer: *const Buffer,
    _proof_is_valid_ptr: *mut bool,
) -> bool {
    false
}

// ── Utils ───────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn key_gen(_ctx: *const RLN, _output_buffer: *mut Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn seeded_key_gen(
    _ctx: *const RLN,
    _input_buffer: *const Buffer,
    _output_buffer: *mut Buffer,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn extended_key_gen(_ctx: *const RLN, _output_buffer: *mut Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn seeded_extended_key_gen(
    _ctx: *const RLN,
    _input_buffer: *const Buffer,
    _output_buffer: *mut Buffer,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn recover_id_secret(
    _ctx: *const RLN,
    _input_proof_buffer_1: *const Buffer,
    _input_proof_buffer_2: *const Buffer,
    _output_buffer: *mut Buffer,
) -> bool {
    false
}

// ── Persistent metadata APIs ────────────────────────────────────────

#[no_mangle]
pub extern "C" fn set_metadata(_ctx: *mut RLN, _input_buffer: *const Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn get_metadata(_ctx: *const RLN, _output_buffer: *mut Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn flush(_ctx: *mut RLN) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn hash(_input_buffer: *const Buffer, _output_buffer: *mut Buffer) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn poseidon_hash(
    _input_buffer: *const Buffer,
    _output_buffer: *mut Buffer,
) -> bool {
    false
}
