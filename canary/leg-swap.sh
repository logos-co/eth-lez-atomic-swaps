#!/usr/bin/env bash
# leg-swap.sh — the full two-peer atomic swap on localnet, end to end. Wraps the
# app's own dogfooded demo (`make demo` == `lgs run --profile demo`, which boots
# the LEZ localnet + Anvil, deploys the HTLC program, and drives a maker/taker
# swap to completion).
#
#   PASS  = exit 0 AND both peers report "completed" with the SAME preimage
#           (the atomicity invariant: one preimage unlocks both chains).
#   FAIL  = the demo ran but a peer refunded / preimages differ / no completion.
#   BROKEN= the toolchain/localnet could not be brought up (scaffold not set up,
#           missing risc0, etc.) — infra, not a swap regression.
#
# HEAVY + toolchain-bound: needs `make setup` (LEZ v0.2.0 toolchain + circuits)
# done first, risc0, and Anvil. On a cold machine this is the slowest leg.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=canary/lib/common.sh
source "$HERE/lib/common.sh"
ROOT="$(canary_repo_root)"
LEG="swap"

for tool in make cargo logos-scaffold; do
  command -v "$tool" >/dev/null 2>&1 || { emit_result "$LEG" broken "$tool not on PATH"; exit $?; }
done

# Stage classification is by EXPLICIT stage evidence, never by the mere presence
# of infra keywords in the log (those words — "localnet", "scaffold", "setup" —
# also appear in SUCCESSFUL bring-up logs, so keyword-presence would misclassify a
# genuine swap regression as a broken canary). Two positive stage boundaries:
#   Stage 1 (leg-owned): `make contracts` (forge build) — a pre-app step whose
#            own exit code we capture; a failure here is toolchain/infra.
#   Stage 2: `make demo` prints "--- Running Swap ---" ONLY after localnet + anvil
#            + HTLC-deploy + wallet setup all succeeded (see src/cli/demo.rs
#            run_demo). Its presence is proof the APP stage was reached, so a
#            nonzero exit AFTER it is a swap-logic failure, not infra.

# --- Stage 1 (pre-app infra): ETH contracts build -------------------------
# A forge/contracts failure is toolchain/infra, never a swap-atomicity
# regression. Run it as its own stage BEFORE the heavy localnet bring-up so its
# exit code classifies cleanly. `make demo` re-runs this (incremental no-op)
# under the Makefile's wallet/circuits env + localnet-stop trap, so we keep
# `make demo` as the single heavy invocation for the swap itself.
canary_log "stage 1/2: building ETH contracts (pre-app infra)"
# BSD/macOS mktemp requires TRAILING X's (a `.log` suffix after them is taken
# literally, so a second/concurrent run collides). Logs are deliberately
# retained (not trap-removed): the result evidence points to them for post-mortem.
CONTRACTS_LOG="$(mktemp "${TMPDIR:-/tmp}/canary-swap-contracts.XXXXXX")"
if ! ( cd "$ROOT" && make contracts ) > "$CONTRACTS_LOG" 2>&1; then
  tail -30 "$CONTRACTS_LOG" >&2
  emit_result "$LEG" broken "contracts (forge) build failed — pre-app infra, not a swap regression — see $CONTRACTS_LOG"
  exit $?
fi

# --- Stage 2: full two-peer swap via 'make demo' --------------------------
canary_log "stage 2/2: running full two-peer swap via 'make demo' (boots localnet + anvil)"
LOG="$(mktemp "${TMPDIR:-/tmp}/canary-swap.XXXXXX")"
( cd "$ROOT" && make demo ) > "$LOG" 2>&1
RC=$?
tail -30 "$LOG" >&2

if [ $RC -ne 0 ]; then
  # Classify by POSITIVE app-stage evidence, not by infra-keyword presence.
  if grep -qF -- "--- Running Swap ---" "$LOG"; then
    # The app stage was reached: localnet + anvil + HTLC-deploy + wallet setup
    # all completed, then the swap itself failed. A real (app) regression.
    emit_result "$LEG" fail "swap app stage reached ('--- Running Swap ---') but 'make demo' exited $RC without a completed swap — see $LOG"
  else
    # Never reached the swap stage: infra/toolchain bring-up (localnet, anvil,
    # HTLC deploy, wallet topup) failed before the app ran.
    emit_result "$LEG" broken "'make demo' failed during infra bring-up before the swap stage (rc=$RC) — see $LOG"
  fi
  exit $?
fi

# Both peers must have completed with the same preimage.
maker_done=$(grep -c "Maker completed" "$LOG")
taker_done=$(grep -c "Taker completed" "$LOG")
# NOTE: `mapfile`/`readarray` is bash 4+; macOS ships bash 3.2, where it would
# error out AFTER the expensive demo already ran. Use a bash-3-compatible
# while-read loop instead.
preimages=()
while IFS= read -r _pre; do
  [ -n "$_pre" ] && preimages+=("$_pre")
done < <(grep -oE "preimage: [0-9a-f]+" "$LOG" | awk '{print $2}' | sort -u)

if [ "$maker_done" -ge 1 ] && [ "$taker_done" -ge 1 ] && [ "${#preimages[@]}" -eq 1 ]; then
  emit_result "$LEG" pass "two-peer swap Completed; both peers share preimage ${preimages[0]:0:16}…"
else
  emit_result "$LEG" fail \
    "swap not atomic: maker_completed=$maker_done taker_completed=$taker_done distinct_preimages=${#preimages[@]} — see $LOG"
fi
exit $?
