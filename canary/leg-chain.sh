#!/usr/bin/env bash
# leg-chain.sh — the golden-path chain leg: replay the exact builder journey on
# a LEZ localnet and assert the sequencer's reactions.
#
#   valid typed transfer        -> assert accepted + included + (funded) balance moves
#   DELIBERATELY-INVALID tx      -> assert LOUDLY REJECTED
#     (bare-u128 authenticated_transfer instruction == bug #640's payload,
#      reconstructed from commit df93c67 / the #640 issue body)
#
# The invalid-tx assertion is EXPECTED TO FAIL today: at v0.2.0 the sequencer
# accepts the malformed instruction, includes it, and the transfer silently
# never executes — no error anywhere (logos-blockchain/logos-execution-zone#640).
# That is the canary's legitimate RED LIGHT, not a canary bug. Exit codes keep
# the two apart (see canary/lib/common.sh): red=10, broken=30.
#
# All heavy lifting is in canary/chain-probe (a standalone Rust binary pinned to
# the same v0.2.0 LEZ deps as the app). This script: builds it, ensures a
# sequencer, runs it (funded, using the debug-genesis keys by default), and maps
# its PROBE_VERDICT to the canary result line.
#
# Configuration (env):
#   CANARY_LEZ_RPC        sequencer to target (default http://127.0.0.1:3040)
#   CANARY_START_LOCALNET =1 to boot a throwaway localnet first (see lib/localnet.sh)
#   CANARY_LEZ_KEY        sender signing key hex (default: debug-genesis key #1)
#   CANARY_LEZ_KEY2       malformed-tx sender key hex (default: debug-genesis key #2)
#   CANARY_UNFUNDED       =1 to skip funding (still proves the red light)
#   CANARY_TARGET_DIR     cargo target dir (default /tmp/canary-target)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=canary/lib/common.sh
source "$HERE/lib/common.sh"
LEG="chain"

RPC="${CANARY_LEZ_RPC:-http://127.0.0.1:3040}"
TARGET_DIR="${CANARY_TARGET_DIR:-/tmp/canary-target}"
PROBE_DIR="$HERE/chain-probe"
PROBE_BIN="$TARGET_DIR/debug/canary-chain-probe"

# Debug-genesis accounts baked into the LEZ v0.2.0 localnet config
# (lez/sequencer/service/configs/debug/sequencer_config.json). Their supply is
# claimable from the vault — see the LEZ repo's `just wallet-import-test-accounts`.
DEFAULT_KEY1="7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb"
DEFAULT_KEY2="f434f8741720014586ae43356d2aec6257da086222f604ddb75d69733b86fc4c"

# 1. Build the probe (fast when the cargo cache is warm) --------------------
# ALWAYS run the incremental build — never trust a pre-existing /tmp binary. The
# probe binary lives in a shared CARGO_TARGET_DIR that persists across runs; if
# the LEZ pin or the probe source changed since a prior run, a stale binary would
# silently produce a verdict for the WRONG code. `cargo build` is a no-op when
# nothing changed (cheap) and rebuilds exactly when it must (correct).
canary_log "building chain-probe (incremental; first build resolves v0.2.0 deps — be patient)"
if ! ( cd "$PROBE_DIR" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build 2>&1 | tail -5 ); then
  emit_result "$LEG" broken "chain-probe failed to compile" ; exit $?
fi
if [ ! -x "$PROBE_BIN" ]; then
  emit_result "$LEG" broken "chain-probe build reported success but $PROBE_BIN is missing"; exit $?
fi

# 2. Optionally boot a throwaway localnet ----------------------------------
STOP_LOCALNET=""
if [ "${CANARY_START_LOCALNET:-0}" = "1" ]; then
  # shellcheck source=canary/lib/localnet.sh
  source "$HERE/lib/localnet.sh"
  if canary_localnet_start; then
    RPC="$CANARY_LOCALNET_RPC"
    STOP_LOCALNET=1
    trap 'canary_localnet_stop' EXIT INT TERM
  else
    emit_result "$LEG" broken "requested localnet boot failed (see lib/localnet.sh output)"
    exit $?
  fi
fi

# 3. Run the probe ----------------------------------------------------------
KEY1="${CANARY_LEZ_KEY:-$DEFAULT_KEY1}"
KEY2="${CANARY_LEZ_KEY2:-$DEFAULT_KEY2}"
declare -a PROBE_ARGS=(run --rpc "$RPC" --amount 5)
if [ "${CANARY_UNFUNDED:-0}" != "1" ]; then
  PROBE_ARGS+=(--funded)
fi

canary_log "probing sequencer $RPC (funded=${CANARY_UNFUNDED:+0}${CANARY_UNFUNDED:-1})"
PROBE_OUT="$(CANARY_LEZ_KEY="$KEY1" CANARY_LEZ_KEY2="$KEY2" \
  "$PROBE_BIN" "${PROBE_ARGS[@]}" 2>&1)"
echo "$PROBE_OUT" >&2

VERDICT_LINE="$(printf '%s\n' "$PROBE_OUT" | grep '^PROBE_VERDICT ' | tail -1)"
if [ -z "$VERDICT_LINE" ]; then
  emit_result "$LEG" broken "chain-probe produced no verdict (sequencer unreachable at $RPC?)"
  exit $?
fi

PAYLOAD="${VERDICT_LINE#PROBE_VERDICT }"
STATUS="${PAYLOAD%%|*}"
EVIDENCE="${PAYLOAD#*|}"
emit_result "$LEG" "$STATUS" "$EVIDENCE"
exit $?
