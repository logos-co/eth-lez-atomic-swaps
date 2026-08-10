#!/usr/bin/env python3
# canary-content-scan.py — content verification for a LOCALLY BUILT .lgx, run
# BEFORE anything is published to the canary channel. This is the local-file
# twin of canary/leg-release-content.sh: same byte-level scan, same
# expectations map, but pointed at a freshly-built artifact on disk instead of
# a published GitHub release asset.
#
# WHY IT EXISTS
#   The canary channel (.github/workflows/canary-channel.yml) publishes branch
#   builds minutes after they compile, with no version bump and no human in the
#   loop. leg-release-content.sh only runs AFTER a release is published, so it
#   is a post-mortem for the canary path. This script moves the same guarantee
#   BEFORE the publish: if a .lgx does not contain the code its channel claims,
#   the workflow fails loudly and nothing reaches the rolling `canary` release.
#
# WHAT IT ASSERTS (per the module's HIGHEST-versioned entry in
# release-content-expectations.json — see --module / the map's _doc):
#   qml_grep         markers must appear in the files of EVERY variant present
#                    in the .lgx (swap_ui ships its QML sources in the artifact)
#   binary_strings   markers must appear in the native plugin libraries
#                    (.so/.dylib) of EVERY variant present — a byte-level
#                    substring scan equivalent to `strings lib | grep -F marker`
#                    but dependency-free (adapted verbatim from
#                    leg-release-content.sh's scan_variant).
#
# DELIBERATE DIFFERENCES FROM leg-release-content.sh:
#   * Local file, not a downloaded release asset (no network, no GitHub API).
#   * Scans ONLY the variants actually present in the .lgx. A canary build is
#     single-platform by default, so requiring all three variants (as the
#     release leg does) would be wrong here — a darwin-only .lgx legitimately
#     carries only variants/darwin-arm64/.
#   * required_ancestors are NOT checked. That marker guards against a release
#     cut from a STALE ref; the canary builds the EXACT ref the operator asked
#     for, so ancestry is not the question. Content markers (is the code in the
#     bytes?) are.
#   * Always uses the HIGHEST mapped version's markers for the module, because
#     the canary's own version string is a sentinel (0.99.<run>) that matches no
#     map entry. "Highest entry" == the newest guarded feature set, which is the
#     strongest content assertion available.
#
# EXIT: 0 = every marker present in every present variant. non-zero = a miss
# (loud, with the exact marker + variant) or a structural problem.
#
# USAGE
#   canary-content-scan.py --lgx PATH --module {swap|swap_ui} \
#       [--expectations FILE] [--manifest-name NAME]
import argparse
import json
import os
import re
import sys
import tarfile
import tempfile

NATIVE_LIB = re.compile(r"\.dylib$|\.so(\.\d+)*$")


def log(msg):
    print(f"[canary-content-scan] {msg}", file=sys.stderr)


def ver_key(v):
    """Loose semver sort key — same convention as leg-release-content.sh."""
    nums = re.findall(r"\d+", v or "")
    return tuple(int(x) for x in nums) if nums else (0,)


def scan_variant(vdir, markers, native_only):
    """Return the subset of `markers` present in the variant's files.
    Byte-level substring scan: for ASCII markers this is exactly what
    `strings <lib> | grep -F <marker>` would find, without needing binutils.
    Lifted from leg-release-content.sh's scan_variant."""
    found = set()
    todo = list(markers)
    for dirpath, _dirs, files in os.walk(vdir):
        for name in files:
            if native_only and not NATIVE_LIB.search(name):
                continue
            with open(os.path.join(dirpath, name), "rb") as fh:
                blob = fh.read()
            for m in todo:
                if m not in found and m.encode() in blob:
                    found.add(m)
        if len(found) == len(todo):
            break
    return found


def pick_highest(entries):
    """The HIGHEST mapped version and its markers for a module."""
    if not entries:
        return None, None
    best = max(entries, key=ver_key)
    return best, entries[best]


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--lgx", required=True, help="path to the built .lgx")
    ap.add_argument("--module", required=True,
                    help="logical module name key in the expectations map "
                         "(swap | swap_ui)")
    here = os.path.dirname(os.path.abspath(__file__))
    ap.add_argument("--expectations",
                    default=os.path.join(here, "release-content-expectations.json"))
    ap.add_argument("--manifest-name", default=None,
                    help="if set, assert the .lgx top-level manifest.json name "
                         "equals this (belt-and-braces check)")
    args = ap.parse_args()

    if not os.path.isfile(args.lgx):
        log(f"FATAL: no such .lgx: {args.lgx}")
        return 2
    with open(args.expectations) as fh:
        expectations = json.load(fh)
    entries = expectations.get("modules", {}).get(args.module)
    if not entries:
        log(f"FATAL: module {args.module!r} has no entry in {args.expectations}")
        return 2

    exp_version, exp = pick_highest(entries)
    qml_markers = exp.get("qml_grep", [])
    bin_markers = exp.get("binary_strings", [])
    log(f"scanning {os.path.basename(args.lgx)} against {args.module}@{exp_version} "
        f"(HIGHEST mapped entry): {len(qml_markers)} qml + {len(bin_markers)} "
        f"binary markers; required_ancestors intentionally skipped for canary")

    problems = []
    with tempfile.TemporaryDirectory(prefix="canary-scan-") as tmp:
        dest = os.path.join(tmp, "x")
        try:
            with tarfile.open(args.lgx, "r:gz") as tf:   # .lgx = gzipped tar
                try:
                    tf.extractall(dest, filter="data")   # py>=3.12 safe-extract
                except TypeError:
                    tf.extractall(dest)
        except Exception as e:
            log(f"FATAL: {args.lgx} is not a valid gzipped tar: {e}")
            return 2

        # Structural: the top-level manifest identifies the module.
        try:
            with open(os.path.join(dest, "manifest.json")) as fh:
                man = json.load(fh)
            log(f"manifest: name={man.get('name')} version={man.get('version')} "
                f"type={man.get('type')}")
            if args.manifest_name and man.get("name") != args.manifest_name:
                problems.append(f"manifest.json name {man.get('name')!r} != "
                                f"expected {args.manifest_name!r}")
        except Exception as e:
            problems.append(f"no readable top-level manifest.json in .lgx: {e}")

        vroot = os.path.join(dest, "variants")
        present = sorted(d for d in os.listdir(vroot)) if os.path.isdir(vroot) else []
        if not present:
            log("FATAL: .lgx carries no variants/ directory")
            return 2
        log(f"variants present in artifact: {', '.join(present)}")

        for variant in present:
            vdir = os.path.join(vroot, variant)
            for markers, native_only, kind in ((qml_markers, False, "qml_grep"),
                                               (bin_markers, True, "binary_strings")):
                if not markers:
                    continue
                found = scan_variant(vdir, markers, native_only)
                for m in markers:
                    ok = m in found
                    log(f"{'PASS' if ok else 'FAIL'} {variant} {kind} {m!r}")
                    if not ok:
                        problems.append(f"{variant}: {kind} marker {m!r} not found in "
                                        f"built artifact")

    if problems:
        log("")
        log(f"CONTENT VERIFICATION FAILED for {args.module} "
            f"(against {args.module}@{exp_version} markers):")
        for p in problems:
            log(f"  - {p}")
        log("Refusing to publish: the built .lgx does not contain the code the "
            "canary channel would claim.")
        return 1

    log(f"CONTENT OK: {args.module} .lgx contains every {exp_version} marker in "
        f"all {len(present)} present variant(s).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
