#!/usr/bin/env bash
# run-all.sh — run the canary legs, aggregate their machine-readable result
# lines, print a summary table (also to the GitHub job summary when present),
# and write a status JSON artifact.
#
# Usage:  canary/run-all.sh [leg ...]      # default: catalog modules chain swap
#   e.g.  canary/run-all.sh catalog modules        # the cheap+fast subset
#
# Env:
#   CANARY_STATUS_JSON   where to write the status artifact (default /tmp/canary-status.json)
#   plus every per-leg var (see each leg script) — passed straight through.
#
# Exit code = the worst severity seen: 0 all pass, 10 a RED light (expected
# ecosystem signal), 20 a real FAIL, 30 a BROKEN leg (canary couldn't run).
# A RED light is deliberately NOT a hard failure — see canary/README.md.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/lib/common.sh"

LEGS=("$@")
[ ${#LEGS[@]} -eq 0 ] && LEGS=(catalog modules chain swap)

# BSD/macOS mktemp requires the X's to be TRAILING (a suffix after them is taken
# literally, breaking a second/concurrent run). Use a trailing-X template and a
# cleanup trap so we never leak the intermediate JSONL.
RESULTS="$(mktemp "${TMPDIR:-/tmp}/canary-results.XXXXXX")"
trap 'rm -f "$RESULTS"' EXIT
export CANARY_RESULT_FILE="$RESULTS"
: > "$RESULTS"

worst=0
for leg in "${LEGS[@]}"; do
  script="$HERE/leg-$leg.sh"
  if [ ! -f "$script" ]; then
    echo "[canary] no such leg: $leg (skipping)" >&2
    continue
  fi
  echo "==================== leg: $leg ====================" >&2
  bash "$script"
  rc=$?
  # Normalize the leg's exit to the documented canary vocabulary {0,10,20,30}.
  # A leg that exits with ANYTHING else (a raw `1`, 127, a signal like 130/143,
  # an uncaught shell error) crashed before/instead of emitting a verdict: treat
  # it as BROKEN(30) and synthesize a result row so the summary reflects it.
  # Aggregating on the RAW code would let such an exit hide behind a red — e.g.
  # a leg dying with `1` after another reported `10` would be masked by numeric
  # max. Normalizing first guarantees broken/fail can never be masked by red.
  case "$rc" in
    0|10|20|30) norm="$rc" ;;
    *)
      echo "[canary] leg '$leg' exited with undocumented code $rc; recording BROKEN" >&2
      emit_result "$leg" broken \
        "leg exited $rc without a documented canary verdict (crash/signal/uncaught error)" 0 >/dev/null
      norm=30
      ;;
  esac
  # Priority broken(30) > fail(20) > red(10) > pass(0): a plain numeric max, but
  # ONLY on the normalized set, so broken/fail is never masked by a red.
  [ "$norm" -gt "$worst" ] && worst="$norm"
done

STATUS_JSON="${CANARY_STATUS_JSON:-/tmp/canary-status.json}"

# Build the summary table + status artifact from the accumulated JSONL.
SUMMARY="$(RESULTS="$RESULTS" STATUS_JSON="$STATUS_JSON" python3 - <<'PY'
import json, os, time

results = []
with open(os.environ["RESULTS"]) as f:
    for line in f:
        line = line.strip()
        if line:
            results.append(json.loads(line))

counts = {"pass": 0, "red": 0, "fail": 0, "broken": 0}
for r in results:
    counts[r["status"]] = counts.get(r["status"], 0) + 1

icon = {"pass": "🟢 pass", "red": "🔴 red", "fail": "❌ fail", "broken": "🟠 broken"}
rows = []
rows.append("| Leg | Status | Duration | Evidence |")
rows.append("|-----|--------|----------|----------|")
for r in results:
    ev = r["evidence"].replace("|", "\\|")
    if len(ev) > 240:
        ev = ev[:237] + "…"
    rows.append(f"| `{r['leg']}` | {icon.get(r['status'], r['status'])} | {r['duration_s']}s | {ev} |")
table = "\n".join(rows)

status = {
    "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "summary": counts,
    "legs": results,
}
with open(os.environ["STATUS_JSON"], "w") as f:
    json.dump(status, f, indent=2)

overall = "🟢 all green"
if counts["broken"]:
    overall = "🟠 canary broken (could not run a leg)"
elif counts["fail"]:
    overall = "❌ failure"
elif counts["red"]:
    overall = "🔴 red light (expected ecosystem signal — see README red-light policy)"

print("## Golden-path canary\n")
print(f"**{overall}** — "
      f"{counts['pass']} pass · {counts['red']} red · {counts['fail']} fail · {counts['broken']} broken\n")
print(table)
PY
)"

echo "$SUMMARY"
echo
echo "[canary] status artifact: $STATUS_JSON"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  echo "$SUMMARY" >> "$GITHUB_STEP_SUMMARY"
fi

exit "$worst"
