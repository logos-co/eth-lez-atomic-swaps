#!/usr/bin/env python3
# canary-index.py — build the canary channel's catalog index, in the SAME shape
# as the official rolling `index` release's index.json (schemaVersion 2), but
# from locally-built .lgx artifacts instead of from published releases.
#
# The official index (logos-modules-release-tool's index.py, uploaded by
# rebuild-index.yml) is an object:
#   { schemaVersion, repositoryName, generatedAt, packages: [
#       { name, versions: [
#           { releasedAt, publisherRef, url, size, sha256, rootHash, manifest }
#       ] } ] }
# where each version entry mirrors the release's sidecar.json. This script
# reproduces exactly that entry shape (see /tmp release sidecar.json for a
# reference) so Basecamp parses a canary index identically to the real one, and
# so canary/leg-catalog.sh's internal-consistency checks
# (manifest.name==package, manifest.version==entry version, url present) hold.
#
# TWO SUBCOMMANDS
#   emit-entry  Given ONE built .lgx + the URL it will live at, emit a single
#               JSON "fragment": {"package_name": ..., "entry": {...}}. One per
#               built (module, platform); the build matrix produces N of these.
#   assemble    Given a directory of fragments, group them by package_name and
#               emit the full index.json. Multiple platform builds of the same
#               module collapse into ONE version entry per module (Basecamp
#               installs the host-matching variant from the multi-variant .lgx;
#               a single-platform canary simply advertises the one variant it
#               built — see canary-channel.yml).
#
# VERSION STRING: this script does NOT invent the version — it reads whatever
# is in the .lgx's own manifest.json. The workflow patches metadata.json to the
# sentinel 0.99.<run-number> BEFORE building, so the .lgx manifest, this index
# entry, and the entry's `version` all agree end-to-end. Rationale for the
# sentinel (vs a semver-prerelease like 0.4.3-canary.<sha>) is documented in
# canary/README.md and the workflow.
#
# USAGE
#   emit-entry --lgx PATH --url URL --publisher-ref REF [--released-at ISO8601]
#              [--out FILE]
#   assemble   --fragments DIR --repository-name NAME [--generated-at ISO8601]
#              [--out FILE]
import argparse
import datetime
import glob
import hashlib
import json
import os
import re
import sys
import tarfile


def now_iso():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def ver_key(v):
    nums = re.findall(r"\d+", v or "")
    return tuple(int(x) for x in nums) if nums else (0,)


def read_lgx_manifest(lgx):
    """Extract and return the top-level manifest.json dict from a .lgx."""
    with tarfile.open(lgx, "r:gz") as tf:
        member = None
        for cand in ("manifest.json", "./manifest.json"):
            try:
                member = tf.getmember(cand)
                break
            except KeyError:
                continue
        if member is None:
            raise KeyError("no top-level manifest.json in .lgx")
        fh = tf.extractfile(member)
        return json.loads(fh.read().decode())


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def cmd_emit_entry(args):
    manifest = read_lgx_manifest(args.lgx)
    name = manifest.get("name")
    version = manifest.get("version")
    if not name or not version:
        print(f"FATAL: .lgx manifest missing name/version: {manifest}", file=sys.stderr)
        return 2
    entry = {
        "releasedAt": args.released_at or now_iso(),
        "publisherRef": args.publisher_ref,
        "url": args.url,
        "size": os.path.getsize(args.lgx),
        "sha256": sha256_file(args.lgx),
        "rootHash": manifest.get("hashes", {}).get("root"),
        "manifest": manifest,
    }
    fragment = {"package_name": name, "entry": entry}
    out = json.dumps(fragment, indent=2)
    if args.out:
        with open(args.out, "w") as fh:
            fh.write(out + "\n")
        print(f"[canary-index] wrote fragment for {name}@{version} -> {args.out}",
              file=sys.stderr)
    else:
        print(out)
    return 0


def cmd_assemble(args):
    fragments = sorted(glob.glob(os.path.join(args.fragments, "*.json")))
    if not fragments:
        print(f"FATAL: no *.json fragments under {args.fragments}", file=sys.stderr)
        return 2
    packages = {}  # name -> {version -> entry}   (dedupe same-version entries)
    for path in fragments:
        with open(path) as fh:
            frag = json.load(fh)
        name = frag["package_name"]
        entry = frag["entry"]
        v = entry.get("manifest", {}).get("version")
        packages.setdefault(name, {})
        # Same module built for several platforms in one run share a version.
        # Keep the first; a single-platform canary has exactly one anyway.
        packages[name].setdefault(v, entry)

    index = {
        "schemaVersion": 2,
        "repositoryName": args.repository_name,
        "generatedAt": args.generated_at or now_iso(),
        "packages": [
            {
                "name": name,
                "versions": [versions[v] for v in
                             sorted(versions, key=ver_key, reverse=True)],
            }
            for name, versions in sorted(packages.items())
        ],
    }
    out = json.dumps(index, indent=2)
    if args.out:
        with open(args.out, "w") as fh:
            fh.write(out + "\n")
        print(f"[canary-index] wrote index with {len(index['packages'])} "
              f"package(s) -> {args.out}", file=sys.stderr)
    else:
        print(out)
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    e = sub.add_parser("emit-entry", help="emit one index fragment for a .lgx")
    e.add_argument("--lgx", required=True)
    e.add_argument("--url", required=True,
                   help="download URL the .lgx will live at on the canary release")
    e.add_argument("--publisher-ref", required=True,
                   help="provenance ref, e.g. canary-<shortsha> or the branch")
    e.add_argument("--released-at", default=None)
    e.add_argument("--out", default=None)
    e.set_defaults(func=cmd_emit_entry)

    a = sub.add_parser("assemble", help="assemble fragments into a full index.json")
    a.add_argument("--fragments", required=True)
    a.add_argument("--repository-name", required=True)
    a.add_argument("--generated-at", default=None)
    a.add_argument("--out", default=None)
    a.set_defaults(func=cmd_assemble)

    args = ap.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
