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
import json, os, re, sys, urllib.request, urllib.error

root = os.environ["ROOT"]

def ver_key(v):
    """Loose semver sort key: the run of integers in the version string.
    Good enough to pick the LATEST release among a package's versions."""
    nums = re.findall(r"\d+", v or "")
    return tuple(int(x) for x in nums) if nums else (0,)

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

index_names = {pkg.get("name") for pkg in pkgs if pkg.get("name")}

# Every module the repo actually ships (its metadata.json is the source of
# truth) MUST appear in the index. A PARTIAL index — e.g. `swap` present but
# `swap_ui` dropped — is a real catalog regression, so require the full expected
# set derived from the repo's metadata.json files, not just "at least one".
missing = sorted(set(truth) - index_names)
if missing:
    die("fail",
        f"index.json is missing expected module(s) {missing} "
        f"(expected={sorted(truth)}, present={sorted(n for n in index_names if n)})")

checked = []
problems = []       # content-level defects (catalog is WRONG)          -> fail(20)
infra_problems = [] # network-inability / unverifiable (can't complete) -> broken(30)
for pkg in pkgs:
    name = pkg.get("name")
    versions = pkg.get("versions", [])

    # metadata.json describes only the LATEST release, so historical versions
    # must NOT be compared against it (a 2nd release would false-fail the 1st).
    # Determine the latest version entry for the eventual metadata cross-check.
    latest_v = None
    for ver in versions:
        v = ver.get("version") or ver.get("manifest", {}).get("version")
        if latest_v is None or ver_key(v) > ver_key(latest_v):
            latest_v = v

    for ver in versions:
        v = ver.get("version") or ver.get("manifest", {}).get("version")
        url = ver.get("url")
        size = ver.get("size")
        man = ver.get("manifest", {})
        # INTERNAL consistency — holds for EVERY historical version:
        #   manifest.name matches the package, manifest.version matches the
        #   version entry, and the .lgx asset is reachable with the right size.
        if man.get("name") and man.get("name") != name:
            problems.append(f"{name}: manifest.name={man.get('name')} != package {name}")
        if man.get("version") and v and man.get("version") != v:
            problems.append(
                f"{name} {v}: manifest.version={man.get('version')} != version entry {v}")
        # HEAD the .lgx asset (follow GitHub's redirect to the CDN).
        # CLASSIFICATION (P2-4/P2-5): only content-level defects are fail(20) —
        # a 404 on an ADVERTISED asset, a size mismatch, a missing index `size`,
        # or a bad manifest. Anything that merely means we COULDN'T CHECK — DNS
        # failure, timeout, connection refused, 5xx, or an unverifiable size (no
        # Content-Length) — is broken(30), a transient harness inability, not a
        # catalog regression.
        if not url:
            problems.append(f"{name} {v}: version entry has no url")  # content: index malformed
            continue
        try:
            code, clen = head_asset(url)
        except urllib.error.HTTPError as e:
            if e.code == 404:
                problems.append(f"{name} {v}: asset HTTP 404 (advertised asset missing) {url}")
            elif e.code and e.code >= 500:
                infra_problems.append(f"{name} {v}: asset server error HTTP {e.code} (transient) {url}")
            else:
                # Other 4xx on an advertised asset is a real catalog/access defect.
                problems.append(f"{name} {v}: asset HTTP {e.code} {url}")
            continue
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            # DNS / timeout / connection refused / socket error — network-inability.
            infra_problems.append(f"{name} {v}: asset unreachable (network) {url} ({e})")
            continue
        except Exception as e:
            # Unknown fetch failure: default to broken, never false-fail.
            infra_problems.append(f"{name} {v}: asset fetch error {url} ({e})")
            continue
        # Got an HTTP response with a status code.
        if code == 404:
            problems.append(f"{name} {v}: asset HTTP 404 (advertised asset missing) {url}")
        elif code and code >= 500:
            infra_problems.append(f"{name} {v}: asset server error HTTP {code} (transient) {url}")
        elif code != 200:
            problems.append(f"{name} {v}: asset HTTP {code} {url}")
        elif size is None:
            # Index entry itself has no size field — a catalog defect.
            problems.append(f"{name} {v}: index entry has no size field")
        elif clen is None:
            # 200 but the server gave no Content-Range/Content-Length — the size
            # is unverifiable, so we cannot complete the check: broken, not fail.
            infra_problems.append(
                f"{name} {v}: asset size unverifiable (no Content-Length from server) {url}")
        elif int(clen) != int(size):
            problems.append(f"{name} {v}: asset size {clen} != index size {size}")
        else:
            checked.append(f"{name}@{v}({int(clen)//1024}KiB ok)")

    # Cross-check ONLY the latest version per package against metadata.json.
    if name in truth:
        if latest_v != truth[name]:
            problems.append(
                f"{name}: latest index version {latest_v} != metadata.json {truth[name]}")
    else:
        # An extra package with no in-repo metadata.json — can't validate its
        # version against the source of truth; note it, don't false-fail.
        checked.append(f"{name}(no in-repo metadata; latest={latest_v} unverified)")

# Content-level defects win: a genuine catalog regression is reported as fail
# even if some other asset was ALSO unverifiable due to a transient error.
if problems:
    die("fail", "; ".join(problems))
# No content defects, but something prevented a COMPLETE verification (network
# down, 5xx, unverifiable size) — inconclusive, so broken (not a false pass).
if infra_problems:
    die("broken", "could not fully verify catalog (transient/unverifiable): " + "; ".join(infra_problems))

die("pass", f"schemaV{index.get('schemaVersion')} chain ok: " + ", ".join(checked))
PY
)"

STATUS="${OUT%%|*}"
EVIDENCE="${OUT#*|}"
[ -z "$STATUS" ] && { STATUS=broken; EVIDENCE="catalog validator produced no output"; }
emit_result "$LEG" "$STATUS" "$EVIDENCE"
exit $?
