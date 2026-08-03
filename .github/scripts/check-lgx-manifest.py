#!/usr/bin/env python3
"""Assert a built .lgx's embedded manifest.json still agrees with metadata.json.

WHY THIS EXISTS
---------------
`swap` 0.3.0 shipped a .lgx whose manifest had no `display_name` and carried
`manifestVersion: "0.2.0"` against `version: "0.3.0"` (issue #60). Nothing
noticed, because a *dropped optional field* fails nothing: the build was green,
`lgx verify` was green, the release published, and the defect only surfaced as a
blank module name in Basecamp's App Manager weeks later.

The mechanism is worth stating, because it will recur:
nix-bundle-lgx's `bundle.sh` copies metadata.json into the manifest through an
explicit key ALLOWLIST. A `flake.lock` pinning a bundler older than a field's
introduction silently drops that field -- the metadata is read fine, the
allowlist just has no entry for it. `swap-module/flake.lock` sat one commit
before logos-co/nix-bundle-lgx@038f9cb ("add support for display_name"), so
every release built from it lost exactly that one key.

So this check compares the two ends of that pipeline directly. If the packaging
toolchain ever again fails to carry a field across, the build goes red on the
PR that introduces it, naming the field.

WHAT IS CHECKED
---------------
1. ROUND-TRIP FIELDS. For every field in ROUND_TRIP_FIELDS that metadata.json
   sets to a truthy value, the manifest must carry an identical value.
   Truthiness mirrors bundle.sh, which patches under `if metadata.get(key)` --
   a falsy metadata value is not copied, so asserting on it would be asserting
   on something the bundler never promised.

   Deliberately NOT compared:
     * `main`  -- metadata holds a base name ("swap_plugin"), the manifest holds
                  a per-variant filename map. Different shapes by design.
     * `icon`  -- rewritten to the bundled basename, or "" when unset.
     * `hashes`, `manifestVersion` -- produced by lgx, not copied from metadata.
     * anything not in bundle.sh's allowlist (`interface`, `nix`, `capabilities`,
       `include`, ...) is not expected to appear in the manifest at all.

2. manifestVersion FLOOR. Hard failure if the package-format version stamped by
   lgx is older than MIN_MANIFEST_VERSION. This is the second half of #60: the
   pinned lgx still had `Manifest::CURRENT_VERSION = "0.2.0"` (bumped to 0.3.0
   in logos-package@18b0075). A floor catches a backwards pin without going
   stale in the false-positive direction -- raise the constant when the
   packaging stack's format version advances and this repo has moved onto it.

3. manifestVersion vs MODULE version era -- SURFACED, NOT FATAL. #60's smell
   test was "manifestVersion 0.2.0 on a 0.3.0 module". It is a good smell but a
   bad assertion: the manifest FORMAT version and the MODULE version are
   independent numbers that only happened to be comparable here. Making it fatal
   would turn `swap` 0.4.0 red against a perfectly current 0.3.0 format. So it
   is printed on every run and annotated as a warning when the eras diverge.

Runs standalone, so the same command that runs in CI can be run by hand:

    python3 .github/scripts/check-lgx-manifest.py \
        --metadata swap-module/metadata.json \
        --lgx /nix/store/...-swap-lgx

`--lgx` accepts the .lgx itself or a directory containing exactly one (which is
what `nix build .#lgx-portable` produces: `$out/<name>.lgx`).

NOTE FOR THE NEXT PERSON: a .lgx is a GZIPPED TAR, not a zip. `tar xzf`, not
`unzip`. This script uses python's tarfile so there is no way to get it wrong.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tarfile

# Fields bundle.sh copies from metadata.json into manifest.json, minus the ones
# it transforms (see module docstring). Keep in step with the allowlist in
# logos-co/nix-bundle-lgx bundle.sh -- if a field is ADDED there, add it here so
# a future stale pin is caught for that field too.
ROUND_TRIP_FIELDS = (
    "name",
    "version",
    "display_name",
    "description",
    "author",
    "type",
    "category",
    "dependencies",
    "view",
)

# Lowest package-format version this repo accepts in a published .lgx.
# 0.3.0 = logos-package@18b0075 onward. Bump when the stack moves on.
MIN_MANIFEST_VERSION = "0.3.0"


def parse_version(text):
    """Best-effort numeric tuple for a dotted version. None if unparseable."""
    if not isinstance(text, str):
        return None
    core = text.split("-", 1)[0].split("+", 1)[0]
    parts = core.split(".")
    try:
        return tuple(int(p) for p in parts)
    except ValueError:
        return None


def resolve_lgx(path):
    if os.path.isdir(path):
        found = sorted(f for f in os.listdir(path) if f.endswith(".lgx"))
        if len(found) != 1:
            die(f"expected exactly one .lgx in {path}, found {found or 'none'}")
        return os.path.join(path, found[0])
    return path


def read_manifest(lgx_path):
    """Extract manifest.json from the .lgx (a gzipped tar)."""
    if not os.path.isfile(lgx_path):
        die(f"no such .lgx: {lgx_path}")
    try:
        with tarfile.open(lgx_path, "r:gz") as tar:
            member = tar.getmember("manifest.json")
            payload = tar.extractfile(member).read()
    except tarfile.ReadError as exc:
        die(
            f"could not read {lgx_path} as a gzipped tar ({exc}). "
            "A .lgx is tar+gzip -- if this looks like a zip, the bundler changed format."
        )
    except KeyError:
        die(f"{lgx_path} contains no manifest.json")
    return json.loads(payload)


def die(message):
    print(f"::error::{message}", file=sys.stderr)
    sys.exit(2)


def annotate(level, message, file=None):
    loc = f" file={file}" if file else ""
    print(f"::{level}{loc}::{message}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--metadata", required=True, help="path to the module's metadata.json")
    ap.add_argument("--lgx", required=True, help="path to the built .lgx, or the dir containing it")
    ap.add_argument("--label", default=None, help="name for this check in log output")
    args = ap.parse_args()

    with open(args.metadata, encoding="utf-8") as handle:
        metadata = json.load(handle)

    lgx_path = resolve_lgx(args.lgx)
    manifest = read_manifest(lgx_path)

    label = args.label or metadata.get("name", "<module>")
    print(f"=== .lgx manifest round-trip check: {label} ===")
    print(f"  metadata : {args.metadata}")
    print(f"  package  : {lgx_path}")
    print(f"  manifest : {json.dumps(manifest, ensure_ascii=False, sort_keys=True)}")
    print()

    failures = []

    # ---- 1. field-for-field round trip ------------------------------------
    for field in ROUND_TRIP_FIELDS:
        expected = metadata.get(field)
        if not expected:
            # Not set in metadata -> bundle.sh does not copy it -> nothing promised.
            print(f"  [skip] {field}: not set in metadata.json")
            continue
        if field not in manifest:
            failures.append(
                f"{field}: DROPPED by the packaging toolchain. metadata.json has "
                f"{json.dumps(expected, ensure_ascii=False)}, the packaged manifest.json "
                f"has no {field!r} key at all. This is the signature of a flake.lock "
                f"pinning a nix-bundle-lgx older than the field (see issue #60): its "
                f"bundle.sh copies manifest fields through a key allowlist, and a field "
                f"missing from that allowlist is dropped without any error. Fix by "
                f"updating the packaging inputs -- "
                f"`cd <module> && nix flake update nix-bundle-lgx logos-module-builder/nix-bundle-lgx` "
                f"-- not by editing the manifest."
            )
            continue
        actual = manifest[field]
        if actual != expected:
            failures.append(
                f"{field}: MISMATCH. metadata.json has "
                f"{json.dumps(expected, ensure_ascii=False)}, packaged manifest.json has "
                f"{json.dumps(actual, ensure_ascii=False)}."
            )
            continue
        print(f"  [ ok ] {field}: {json.dumps(actual, ensure_ascii=False)}")

    # ---- 2. manifestVersion floor -----------------------------------------
    manifest_version = manifest.get("manifestVersion")
    print()
    print(f"  manifestVersion : {manifest_version!r}  (floor {MIN_MANIFEST_VERSION})")
    mv = parse_version(manifest_version)
    floor = parse_version(MIN_MANIFEST_VERSION)
    if manifest_version is None:
        failures.append(
            "manifestVersion: absent from the packaged manifest.json. lgx always stamps "
            "Manifest::CURRENT_VERSION, so an absent value means this .lgx was not "
            "produced by the expected toolchain."
        )
    elif mv is None:
        failures.append(f"manifestVersion: {manifest_version!r} is not a parseable dotted version.")
    elif mv < floor:
        failures.append(
            f"manifestVersion: {manifest_version} is OLDER than the {MIN_MANIFEST_VERSION} "
            f"floor this repo requires. The packaging toolchain in flake.lock is behind: "
            f"lgx stamps Manifest::CURRENT_VERSION, which reached {MIN_MANIFEST_VERSION} in "
            f"logos-package@18b0075. A stale stamp breaks `lgx merge` interop (compareMetadata "
            f"treats a manifestVersion mismatch as an error) and, per issue #60, travels with "
            f"a bundler old enough to drop manifest fields silently. Update the packaging "
            f"inputs in flake.lock."
        )

    # ---- 3. era comparison (surfaced, not fatal) --------------------------
    module_version = metadata.get("version")
    mod = parse_version(module_version)
    if mv is not None and mod is not None:
        mv_era, mod_era = mv[:2], mod[:2]
        print(f"  module version  : {module_version!r}  (era {'.'.join(map(str, mod_era))})")
        if mv_era < mod_era:
            annotate(
                "warning",
                f"{label}: manifestVersion {manifest_version} is from an older era than the "
                f"module version {module_version}. Not fatal on its own -- the manifest FORMAT "
                f"version and the MODULE version are independent -- but in issue #60 this exact "
                f"skew was the visible symptom of a three-month-stale packaging pin. Confirm "
                f"flake.lock's nix-bundle-lgx / logos-package are current before releasing.",
                file=args.metadata,
            )

    print()
    if failures:
        print(f"FAILED: packaged manifest.json does not match {args.metadata}")
        print()
        for failure in failures:
            print(f"  * {failure}")
            annotate("error", f"{label}: {failure}", file=args.metadata)
        print()
        print(
            "  The .lgx is a gzipped tar; inspect it yourself with:\n"
            f"    tar -xzOf {lgx_path} manifest.json | python3 -m json.tool"
        )
        return 1

    print(f"OK: every round-trip field in {args.metadata} survived packaging.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
