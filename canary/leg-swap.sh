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

canary_log "running full two-peer swap via 'make demo' (this boots localnet + anvil)"
# BSD/macOS mktemp requires TRAILING X's (a `.log` suffix after them is taken
# literally, so a second/concurrent run collides). This log is deliberately
# retained (not trap-removed): the result evidence points to it for post-mortem.
LOG="$(mktemp "${TMPDIR:-/tmp}/canary-swap.XXXXXX")"
( cd "$ROOT" && make demo ) > "$LOG" 2>&1
RC=$?
tail -30 "$LOG" >&2

if [ $RC -ne 0 ]; then
  # Distinguish "infra never came up" from "swap logic failed".
  if grep -qiE "localnet|scaffold|setup|no such file|connection refused|toolchain" "$LOG" \
     && ! grep -qi "completed" "$LOG"; then
    emit_result "$LEG" broken "make demo failed to bring up localnet/toolchain (rc=$RC) — see $LOG"
  else
    emit_result "$LEG" fail "make demo exited $RC without a completed swap — see $LOG"
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
