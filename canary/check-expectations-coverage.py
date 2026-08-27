#!/usr/bin/env python3
# check-expectations-coverage.py — assert release-content-expectations.json has
# an EXACT entry for the version each module currently declares in its
# metadata.json.
#
# WHY: issue #165. The map stopped at 0.4.2 while 0.4.3, 0.4.4 and 0.4.5
# shipped. leg-release-content.sh fell back to 0.4.2's markers (two of which
# described UI text #140/#145 had already replaced), so every release since
# 0.4.2 was graded against a stale description and FAILED — and nothing in CI
# said "you cut a version without describing it". This check makes that gap
# a PR-time failure: bump metadata.json and the map in the same change.
#
# Cheap and dependency-free (python3 stdlib). Runs from any cwd.
#
# EXIT 0 = every module's metadata.json version has an exact map entry.
#      1 = at least one is missing (message names the module + version).
#      2 = a file could not be read.
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

# expectations-map key -> the metadata.json that declares its version
MODULES = {
    "swap": os.path.join(ROOT, "swap-module", "metadata.json"),
    "swap_ui": os.path.join(ROOT, "swap-ui", "metadata.json"),
}


def main():
    exp_path = os.path.join(HERE, "release-content-expectations.json")
    try:
        with open(exp_path) as fh:
            entries = json.load(fh).get("modules", {})
    except Exception as e:
        print(f"[expectations-coverage] FATAL: cannot read {exp_path}: {e}", file=sys.stderr)
        return 2

    missing = []
    for module, meta_path in MODULES.items():
        try:
            with open(meta_path) as fh:
                version = json.load(fh)["version"]
        except Exception as e:
            print(f"[expectations-coverage] FATAL: cannot read {meta_path}: {e}", file=sys.stderr)
            return 2
        have = sorted(entries.get(module, {}))
        if version in entries.get(module, {}):
            print(f"[expectations-coverage] ok   {module}@{version} has an exact entry")
        else:
            missing.append((module, version, have))

    if missing:
        for module, version, have in missing:
            print(f"[expectations-coverage] FAIL {module}@{version}: no entry in "
                  f"canary/release-content-expectations.json (mapped: {have or 'none'})",
                  file=sys.stderr)
        print("[expectations-coverage] add an entry for the version in metadata.json "
              "describing what it actually ships; do not rely on the older-version "
              "fallback (see issue #165)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
