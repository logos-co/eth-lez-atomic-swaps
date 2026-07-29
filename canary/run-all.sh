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

RESULTS="$(mktemp /tmp/canary-results.XXXXXX.jsonl)"
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
  [ "$rc" -gt "$worst" ] && worst="$rc"
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
