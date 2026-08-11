#!/usr/bin/env bash
# leg-index-fence.sh — the loud backstop for the canary→official-index leak.
#
# BACKGROUND. The official catalogue's index.json is rebuilt by
# rebuild-index.yml, which delegates to logos-modules-release-action's reusable
# rebuild-index. That tool walks EVERY non-draft release on this repo (only the
# rolling `index` tag is skipped) and collects assets matching
# `.name | endswith(".lgx")`. The `canary` prerelease is neither a draft nor
# `index`, so before this PR its `.lgx` assets were slurped straight into the
# OFFICIAL index.json — the 0.99.x builds leaked into the catalogue trial users
# install from.
#
# The PRIMARY, structural fence lives in canary-channel.yml: canary assets are
# published as `.lgxc`, which the official enumerator's `endswith(".lgx")`
# filter skips. THIS leg is the defence-in-depth backstop: it fetches the LIVE
# official index.json and FAILS if a canary build ever appears in it again —
# whether because someone renamed the asset back to `.lgx`, upstream broadened
# the enumerator, or any other regression. Catch it here, loudly, before a
# trial user is offered an unstable 0.99.x build.
#
# WHAT IT ASSERTS (any hit => fail):
#   * no version string in the official index matches the canary sentinel
#     `0.99.<n>` (the version scheme canary-channel.yml stamps);
#   * no entry `url` points at this repo's `canary` release download path
#     (`/releases/download/canary/`), regardless of the asset's extension.
#
# PASS  = the official index is clean of both signals.
# FAIL  = a canary sentinel version or a canary-release URL is present — a real
#         leak of the canary channel into the official catalogue.
# BROKEN= could not fetch/parse the official index (network/tooling), so the
#         check is inconclusive — never a false pass.
#
# The official index URL is read from logos-repo.json's `indexUrl` (same source
# of truth Basecamp uses). Override with CANARY_OFFICIAL_INDEX_URL to point at a
# fixture or a PR-built index — the value may be an http(s):// URL, a file://
# URL, or a plain local path (used by this leg's own negative test).
#
# Cheap, network-only, no toolchain. Runs on any platform.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=canary/lib/common.sh
source "$HERE/lib/common.sh"
ROOT="$(canary_repo_root)"
LEG="index-fence"

REPO_JSON="$ROOT/logos-repo.json"
if [ ! -f "$REPO_JSON" ]; then
  emit_result "$LEG" broken "logos-repo.json not found at repo root" ; exit $?
fi

canary_log "checking the official index is free of any canary (0.99.x / canary-release) asset"

OUT="$(ROOT="$ROOT" python3 - <<'PY'
import json, os, re, sys, urllib.request, urllib.error

root = os.environ["ROOT"]

def die(status, evidence):
    print(f"{status}|{evidence}")
    sys.exit(0)

# Resolve the official index location. Default: logos-repo.json's indexUrl
# (exactly what Basecamp fetches). Override for tests/PR indexes.
override = os.environ.get("CANARY_OFFICIAL_INDEX_URL", "").strip()
if override:
    src = override
else:
    try:
        with open(os.path.join(root, "logos-repo.json")) as f:
            src = json.load(f).get("indexUrl")
    except Exception as e:
        die("broken", f"could not read logos-repo.json: {e}")
    if not src:
        die("broken", "logos-repo.json has no indexUrl")

# Load the index from an http(s)/file URL or a plain local path.
def load_index(s):
    if re.match(r"^https?://", s) or s.startswith("file://"):
        req = urllib.request.Request(s, headers={"User-Agent": "logos-canary/1"})
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read().decode())
    with open(s) as f:  # plain local path
        return json.load(f)

try:
    index = load_index(src)
except (urllib.error.URLError, TimeoutError, OSError) as e:
    die("broken", f"could not fetch official index ({src}): {e}")
except Exception as e:
    die("broken", f"could not parse official index ({src}): {e}")

# The canary sentinel is 0.99.<run-number> (see canary-channel.yml). Match the
# whole 0.99.<n> family, not a bare "0.99" substring, so a legitimate future
# 0.99.x REAL release would be an explicit, deliberate decision — but today any
# 0.99.x in the OFFICIAL index is the canary leak by construction.
SENTINEL = re.compile(r"^0\.99\.\d+")
CANARY_URL = "/releases/download/canary/"

hits = []
for pkg in index.get("packages", []):
    name = pkg.get("name")
    for ver in pkg.get("versions", []):
        v = ver.get("version") or ver.get("manifest", {}).get("version") or ""
        url = ver.get("url") or ""
        if SENTINEL.match(v.strip()):
            hits.append(f"{name}: canary sentinel version {v!r} in official index")
        if CANARY_URL in url:
            hits.append(f"{name} {v}: entry url points at the canary release ({url})")

if hits:
    die("fail", "CANARY LEAKED INTO OFFICIAL INDEX — " + "; ".join(hits))

npkg = len(index.get("packages", []))
nver = sum(len(p.get("versions", [])) for p in index.get("packages", []))
die("pass", f"official index clean: {npkg} package(s), {nver} version(s), "
            f"no 0.99.x sentinel and no canary-release URL")
PY
)"

STATUS="${OUT%%|*}"
EVIDENCE="${OUT#*|}"
[ -z "$STATUS" ] && { STATUS=broken; EVIDENCE="index-fence validator produced no output"; }
emit_result "$LEG" "$STATUS" "$EVIDENCE"
exit $?
