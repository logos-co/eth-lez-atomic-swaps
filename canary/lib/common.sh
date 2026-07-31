#!/usr/bin/env bash
# canary/lib/common.sh — shared helpers for the golden-path canary legs.
#
# Every leg script sources this and, exactly once, calls `emit_result` with a
# machine-readable summary. The summary is a single JSON object printed on its
# own line, prefixed with the marker `CANARY_RESULT ` so the orchestrator (and
# the CI job-summary builder) can grep it out of otherwise-noisy leg output:
#
#   CANARY_RESULT {"leg":"catalog","status":"pass","evidence":"...","duration_s":3}
#
# Status vocabulary (also encoded in the leg's exit code, see below):
#   pass    — the leg proved what it set out to prove.            exit 0
#   red     — an EXPECTED ecosystem red light (a known upstream   exit 10
#             bug reproduced, e.g. #640). NOT a canary defect;
#             this is the canary doing its job.
#   fail    — the leg's assertion failed unexpectedly.            exit 20
#   broken  — the harness itself could not run the check          exit 30
#             (missing toolchain, no localnet, network down).
#
# The distinct `red` vs `broken`/`fail` exit codes are the whole point: a red
# light is signal, a broken canary is noise. CI must treat them differently.
set -uo pipefail

CANARY_STATUS_PASS=0
CANARY_STATUS_RED=10
CANARY_STATUS_FAIL=20
CANARY_STATUS_BROKEN=30

# Monotonic-ish start stamp; `canary_elapsed` returns whole seconds since it.
CANARY_T0="$(date +%s)"

canary_elapsed() {
  local now
  now="$(date +%s)"
  echo $(( now - CANARY_T0 ))
}

# Structured log line to stderr so it never pollutes the result line on stdout.
canary_log() { echo "[canary] $*" >&2; }

# emit_result LEG STATUS EVIDENCE [DURATION_S]
# Prints the marker line to stdout and, if CANARY_RESULT_FILE is set, appends
# the bare JSON object to it. Returns the exit code that matches STATUS so a
# leg can `exit "$(emit_result ...)"`-style — but the canonical usage is:
#   emit_result "$LEG" pass "evidence"; exit $?
emit_result() {
  local leg="$1" status="$2" evidence="$3" dur="${4:-$(canary_elapsed)}"
  local json
  json="$(LEG="$leg" STATUS="$status" EVIDENCE="$evidence" DUR="$dur" python3 - <<'PY'
import json, os
print(json.dumps({
    "leg": os.environ["LEG"],
    "status": os.environ["STATUS"],
    "evidence": os.environ["EVIDENCE"],
    "duration_s": int(os.environ["DUR"]),
}, separators=(",", ":")))
PY
)"
  echo "CANARY_RESULT ${json}"
  if [ -n "${CANARY_RESULT_FILE:-}" ]; then
    printf '%s\n' "$json" >> "$CANARY_RESULT_FILE"
  fi
  case "$status" in
    pass)   return $CANARY_STATUS_PASS ;;
    red)    return $CANARY_STATUS_RED ;;
    fail)   return $CANARY_STATUS_FAIL ;;
    broken) return $CANARY_STATUS_BROKEN ;;
    *)      return $CANARY_STATUS_FAIL ;;
  esac
}

# Resolve the repo root (canary/ lives directly under it).
canary_repo_root() {
  cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd
}
