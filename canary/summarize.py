#!/usr/bin/env python3
"""Merge canary status JSON artifacts into a single markdown summary.

Usage: summarize.py <dir>   # dir contains canary-status-*.json (possibly nested
                            #  under per-artifact subdirs, as download-artifact lays them out)
Prints a combined table to stdout (intended for $GITHUB_STEP_SUMMARY).
"""
import json
import os
import sys

ICON = {"pass": "🟢 pass", "red": "🔴 red", "fail": "❌ fail", "broken": "🟠 broken"}


def main() -> int:
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    legs = []
    for dirpath, _dirs, files in os.walk(root):
        for fn in files:
            if fn.startswith("canary-status") and fn.endswith(".json"):
                try:
                    with open(os.path.join(dirpath, fn)) as f:
                        legs.extend(json.load(f).get("legs", []))
                except (OSError, ValueError):
                    continue

    counts = {"pass": 0, "red": 0, "fail": 0, "broken": 0}
    for leg in legs:
        counts[leg["status"]] = counts.get(leg["status"], 0) + 1

    if counts["broken"]:
        overall = "🟠 canary broken (a leg could not run)"
    elif counts["fail"]:
        overall = "❌ failure"
    elif counts["red"]:
        overall = "🔴 red light (expected ecosystem signal — see canary/README.md)"
    elif legs:
        overall = "🟢 all green"
    else:
        overall = "⚪ no results"

    print("## Golden-path canary\n")
    print(
        f"**{overall}** — {counts['pass']} pass · {counts['red']} red · "
        f"{counts['fail']} fail · {counts['broken']} broken\n"
    )
    print("| Leg | Status | Duration | Evidence |")
    print("|-----|--------|----------|----------|")
    for leg in sorted(legs, key=lambda x: x.get("leg", "")):
        ev = leg["evidence"].replace("|", "\\|")
        if len(ev) > 240:
            ev = ev[:237] + "…"
        print(f"| `{leg['leg']}` | {ICON.get(leg['status'], leg['status'])} | {leg['duration_s']}s | {ev} |")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
