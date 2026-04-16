//! Empty stub for the `rln` crate.
//!
//! The upstream `rln 0.3.4` (transitive dep of `waku-bindings`) pins
//! many of its deps (`ark-serialize`, `thiserror`, etc.) to `=` exact
//! versions that conflict with our LEZ deps. Patching each `=` to a
//! relaxed range was a whack-a-mole fight.
//!
//! Real reason we can stub: `waku-bindings/src/lib.rs` only does a
//! `use rln;` (no calls) just so `libwaku` can statically resolve RLN
//! symbols. Since we don't enable `rln_relay` in `WakuNodeConfig`, the
//! Nim side never actually calls into RLN code, so the symbols don't
//! need to do anything.
//!
//! If you ever need RLN: swap this stub out for the real crate (and
//! deal with the dep-graph fallout).
