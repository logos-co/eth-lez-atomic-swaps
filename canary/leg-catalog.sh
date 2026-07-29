#!/usr/bin/env bash
# leg-catalog.sh — proves the LIVE release catalog chain is intact and
# self-consistent, exactly as a Basecamp client would traverse it:
#
#   logos-repo.json (committed) --indexUrl--> index.json (GitHub release)
#     --> each package version's .lgx asset URL (HEAD, assert reachable + size)
#     --> manifest {name,version} cross-checked against the module's
#         in-repo metadata.json (single source of truth for name/version)
#
# PASS  = index reachable, every .lgx HEADs 200 with matching size, and every
#         package name/version agrees with metadata.json.
# FAIL  = a name/version mismatch or a missing/short asset (catalog is wrong).
# BROKEN= network/tooling failure — could not fetch the index at all.
#
# Cheap, network-only, no toolchain. Runs on any platform.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=canary/lib/common.sh
source "$HERE/lib/common.sh"
ROOT="$(canary_repo_root)"
LEG="catalog"

REPO_JSON="$ROOT/logos-repo.json"
if [ ! -f "$REPO_JSON" ]; then
  emit_result "$LEG" broken "logos-repo.json not found at repo root" ; exit $?
fi

canary_log "traversing catalog chain from $REPO_JSON"

# The whole traversal + validation runs in python3 (JSON + urllib, no jq/curl
# dependency). It prints exactly one line to stdout: STATUS|EVIDENCE.
OUT="$(ROOT="$ROOT" python3 - <<'PY'
import json, os, sys, urllib.request, urllib.error

root = os.environ["ROOT"]

def die(status, evidence):
    print(f"{status}|{evidence}")
    sys.exit(0)

def fetch(url, method="GET", extra=None):
    headers = {"User-Agent": "logos-canary/1"}
    if extra:
        headers.update(extra)
    req = urllib.request.Request(url, method=method, headers=headers)
    return urllib.request.urlopen(req, timeout=30)

def head_asset(url):
    """Return (code, content_length) without downloading the body.
    HEAD first; if the CDN rejects it, fall back to a 1-byte Range GET and read
    the total size from Content-Range."""
    try:
        with fetch(url, method="HEAD") as r:
            return r.status, r.headers.get("Content-Length")
    except urllib.error.HTTPError as e:
        if e.code not in (403, 405, 400):
            raise
    with fetch(url, method="GET", extra={"Range": "bytes=0-0"}) as r:
        cr = r.headers.get("Content-Range")  # e.g. "bytes 0-0/10449756"
        total = cr.split("/")[-1] if cr else r.headers.get("Content-Length")
        return (200 if r.status in (200, 206) else r.status), total

# 1. logos-repo.json -> indexUrl
with open(os.path.join(root, "logos-repo.json")) as f:
    repo = json.load(f)
index_url = repo.get("indexUrl")
if not index_url:
    die("fail", "logos-repo.json has no indexUrl")

# 2. fetch index.json
try:
    with fetch(index_url) as r:
        index = json.loads(r.read().decode())
except Exception as e:  # network / 404 => harness can't run the check
    die("broken", f"could not fetch index.json ({index_url}): {e}")

# 3. in-repo metadata.json = single source of truth for name/version
truth = {}
for sub in ("swap-module", "swap-ui"):
    p = os.path.join(root, sub, "metadata.json")
    try:
        with open(p) as f:
            m = json.load(f)
        truth[m["name"]] = m["version"]
    except Exception as e:
        die("broken", f"could not read {sub}/metadata.json: {e}")

pkgs = index.get("packages", [])
if not pkgs:
    die("fail", "index.json has zero packages")

checked = []
problems = []
for pkg in pkgs:
    name = pkg.get("name")
    for ver in pkg.get("versions", []):
        v = ver.get("version") or ver.get("manifest", {}).get("version")
        url = ver.get("url")
        size = ver.get("size")
        man = ver.get("manifest", {})
        # manifest name/version must agree with the package + metadata.json
        if man.get("name") and man.get("name") != name:
            problems.append(f"{name}: manifest.name={man.get('name')} != package {name}")
        if name in truth and v and truth[name] != v:
            problems.append(f"{name}: index version {v} != metadata.json {truth[name]}")
        elif name not in truth:
            problems.append(f"{name}: no in-repo metadata.json to validate against")
        # HEAD the .lgx asset (follow GitHub's redirect to the CDN)
        if not url:
            problems.append(f"{name} {v}: version entry has no url")
            continue
        try:
            code, clen = head_asset(url)
            if code != 200:
                problems.append(f"{name} {v}: asset HTTP {code}")
            elif size and clen and int(clen) != int(size):
                problems.append(f"{name} {v}: asset size {clen} != index size {size}")
            else:
                checked.append(f"{name}@{v}({(int(clen)//1024) if clen else '?'}KiB ok)")
        except Exception as e:
            problems.append(f"{name} {v}: asset unreachable {url} ({e})")

if problems:
    die("fail", "; ".join(problems))

die("pass", f"schemaV{index.get('schemaVersion')} chain ok: " + ", ".join(checked))
PY
)"

STATUS="${OUT%%|*}"
EVIDENCE="${OUT#*|}"
[ -z "$STATUS" ] && { STATUS=broken; EVIDENCE="catalog validator produced no output"; }
emit_result "$LEG" "$STATUS" "$EVIDENCE"
exit $?
