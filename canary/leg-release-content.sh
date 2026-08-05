#!/usr/bin/env bash
# leg-release-content.sh — proves a PUBLISHED release artifact actually contains
# the code its version string claims. This catches the stale-ref release
# failure that has happened twice and that leg-catalog.sh structurally cannot
# see (it HEADs assets but never downloads a byte):
#
#   * swap_ui 0.3.0 shipped without the History tab
#   * swap_ui 0.3.3 shipped without PR #94's trial-feedback flow
#
# Both times the version string was right and every catalog check passed: the
# release workflow (workflow_dispatch-only) had been dispatched on a stale ref,
# so the tag pointed at an old commit and the .lgx silently lacked recently
# merged code. This leg downloads the released .lgx (a gzipped tar:
# manifest.json + variants/<variant>/...), extracts it, and asserts the
# committed content markers from canary/release-content-expectations.json for
# EVERY shipped variant (darwin-arm64, linux-amd64, linux-arm64 — kept in
# lockstep with the `variants:` line of release-swap*.yml):
#
#   qml_grep           marker must appear in the files extracted for the
#                      variant (swap_ui ships its QML sources in the .lgx)
#   binary_strings     marker must appear in the variant's native plugin
#                      libraries (.so/.dylib) — a byte-level scan, equivalent
#                      to `strings lib | grep -F marker` but dependency-free
#   required_ancestors commit must be an ancestor of the release tag's target
#                      commit — `git merge-base --is-ancestor` when the local
#                      history has both commits (fetched on demand), else the
#                      GitHub compare API
#
# Versions without an exact map entry fall back to the HIGHEST mapped version
# <= the released one (markers accumulate), so an unmapped 0.4.1 still asserts
# 0.4.0's markers instead of passing vacuously; see the map's _doc.
#
# PASS  = every marker present in every variant + every ancestor confirmed.
# FAIL  = a marker or ancestor missing — the released CONTENT is stale/wrong.
# BROKEN= network/API/tooling failure — could not complete the verification.
#
# Env:
#   CANARY_RELEASE_MODULE        swap | swap_ui | all          (default all)
#   CANARY_RELEASE_VERSION       e.g. "0.4.0"; empty = latest published
#                                release (only honored for a single module)
#   CANARY_RELEASE_REPO          owner/repo of the releases to verify
#                                (default logos-co/eth-lez-atomic-swaps)
#   CANARY_RELEASE_EXPECTATIONS  alternate expectations file (default the
#                                committed canary/release-content-expectations.json)
#   GITHUB_TOKEN / GH_TOKEN      optional; authenticates GitHub API calls
#                                (rate-limit headroom in CI)
#
# Cheap, network-only, no toolchain (python3 stdlib + git). Runs on any OS.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=canary/lib/common.sh
source "$HERE/lib/common.sh"
ROOT="$(canary_repo_root)"
LEG="release-content"

EXPECTATIONS="${CANARY_RELEASE_EXPECTATIONS:-$HERE/release-content-expectations.json}"
if [ ! -f "$EXPECTATIONS" ]; then
  emit_result "$LEG" broken "expectations file not found: $EXPECTATIONS" ; exit $?
fi

canary_log "verifying published release content against $EXPECTATIONS"

# The whole resolve + download + extract + assert pipeline runs in python3
# (JSON + urllib + tarfile, no curl/jq/binutils dependency). Per-assertion
# PASS/FAIL lines go to stderr; stdout carries exactly one line: STATUS|EVIDENCE.
OUT="$(ROOT="$ROOT" EXPECTATIONS="$EXPECTATIONS" \
      MODULE="${CANARY_RELEASE_MODULE:-all}" \
      VERSION="${CANARY_RELEASE_VERSION:-}" \
      REPO="${CANARY_RELEASE_REPO:-logos-co/eth-lez-atomic-swaps}" \
      python3 - <<'PY'
import json, os, re, shutil, subprocess, sys, tarfile, tempfile
import urllib.request, urllib.error

root = os.environ["ROOT"]
repo = os.environ["REPO"]
api_base = f"https://api.github.com/repos/{repo}"
token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN") or ""

# The three variants every release must carry — kept in lockstep with the
# `variants:` line in release-swap.yml / release-swap-ui.yml.
VARIANTS = ("darwin-arm64", "linux-amd64", "linux-arm64")

def log(msg):
    print(f"[release-content] {msg}", file=sys.stderr)

def die(status, evidence):
    print(f"{status}|{evidence}")
    sys.exit(0)

def ver_key(v):
    """Loose semver sort key (same convention as leg-catalog.sh)."""
    nums = re.findall(r"\d+", v or "")
    return tuple(int(x) for x in nums) if nums else (0,)

def fetch(url, timeout=60):
    headers = {"User-Agent": "logos-canary/1"}
    # Only authenticate api.github.com calls; asset downloads redirect to a
    # CDN that must NOT see the token.
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, headers=headers)
    return urllib.request.urlopen(req, timeout=timeout)

def api_json(url):
    with fetch(url) as r:
        return json.loads(r.read().decode())

# --- expectations map -------------------------------------------------------
with open(os.environ["EXPECTATIONS"]) as f:
    expectations = json.load(f)
exp_modules = expectations.get("modules", {})

want = os.environ["MODULE"]
modules = sorted(exp_modules) if want in ("", "all") else [want]
pinned_version = os.environ["VERSION"]
if pinned_version and len(modules) != 1:
    die("broken",
        f"CANARY_RELEASE_VERSION={pinned_version} needs a single CANARY_RELEASE_MODULE, got {modules}")

def pick_expectations(module, version):
    """Exact entry, else the HIGHEST mapped version <= the released version
    (markers accumulate — see the map's _doc). None if the release predates
    every mapped version."""
    entries = exp_modules.get(module, {})
    if version in entries:
        return version, entries[version]
    candidates = [v for v in entries if ver_key(v) <= ver_key(version)]
    if not candidates:
        return None, None
    best = max(candidates, key=ver_key)
    return best, entries[best]

# --- GitHub release / tag helpers -------------------------------------------
def latest_version(module):
    """Highest published (non-draft) release version whose tag matches
    <module>-v<semver>. Anchored, so swap-v* never matches swap_ui-v*."""
    rels = api_json(f"{api_base}/releases?per_page=100")
    pat = re.compile(rf"^{re.escape(module)}-v(\d+(?:\.\d+)*)$")
    versions = [m.group(1) for r in rels if not r.get("draft")
                for m in [pat.match(r.get("tag_name") or "")] if m]
    return max(versions, key=ver_key) if versions else None

def tag_target_commit(tag):
    """The commit the release tag points at (dereferencing annotated tags)."""
    obj = api_json(f"{api_base}/git/ref/tags/{tag}")["object"]
    if obj["type"] == "tag":
        obj = api_json(f"{api_base}/git/tags/{obj['sha']}")["object"]
    return obj["sha"]

def git(*args):
    return subprocess.run(["git", "-C", root, *args],
                          capture_output=True, text=True, timeout=300)

def is_ancestor(anc, target):
    """(bool, how) — prefer local `git merge-base --is-ancestor`, fetching the
    missing commits on demand (GitHub serves reachable full-SHA fetches); fall
    back to the compare API when local git history cannot answer.
    Raises on infra failure (both mechanisms unavailable)."""
    if shutil.which("git"):
        try:
            def have(sha):
                return git("cat-file", "-e", f"{sha}^{{commit}}").returncode == 0
            for sha in (anc, target):
                if not have(sha):
                    git("fetch", "--quiet", "origin", sha)  # best-effort
            if (not have(anc) or not have(target)) and \
               git("rev-parse", "--is-shallow-repository").stdout.strip() == "true":
                git("fetch", "--quiet", "--unshallow", "origin")  # best-effort
            if have(anc) and have(target):
                rc = git("merge-base", "--is-ancestor", anc, target).returncode
                if rc in (0, 1):
                    return rc == 0, "git"
        except Exception:
            pass  # fall through to the API
    try:
        cmp = api_json(f"{api_base}/compare/{anc}...{target}")
        return cmp.get("status") in ("ahead", "identical"), "api"
    except urllib.error.HTTPError as e:
        if e.code == 404:
            # An unknown SHA cannot be an ancestor — a content-level miss
            # (typo in the map, or a rewritten/garbage-collected commit).
            return False, "api(404: unknown commit)"
        raise

# --- artifact scanning ------------------------------------------------------
NATIVE_LIB = re.compile(r"\.dylib$|\.so(\.\d+)*$")

def scan_variant(vdir, markers, native_only):
    """Return the subset of `markers` present in the variant's files.
    Byte-level substring scan: for ASCII markers this is exactly what
    `strings <lib> | grep -F <marker>` would find, without needing binutils."""
    found = set()
    todo = [m for m in markers]
    for dirpath, _dirs, files in os.walk(vdir):
        for name in files:
            if native_only and not NATIVE_LIB.search(name):
                continue
            with open(os.path.join(dirpath, name), "rb") as f:
                blob = f.read()
            for m in todo:
                if m not in found and m.encode() in blob:
                    found.add(m)
        if len(found) == len(todo):
            break
    return found

# --- main loop --------------------------------------------------------------
checked = []        # human evidence per module
problems = []       # content-level defects (release is WRONG)      -> fail(20)
infra_problems = [] # network/API/tooling inability (can't verify)  -> broken(30)

for module in modules:
    try:
        version = pinned_version or latest_version(module)
    except Exception as e:
        infra_problems.append(f"{module}: could not list releases: {e}")
        continue
    if not version:
        infra_problems.append(f"{module}: no published release matching {module}-v*")
        continue
    tag = f"{module}-v{version}"
    log(f"--- {module}@{version} (release tag {tag}) ---")

    try:
        release = api_json(f"{api_base}/releases/tags/{tag}")
    except urllib.error.HTTPError as e:
        if e.code == 404:
            problems.append(f"{module} {version}: release tag {tag} does not exist")
        else:
            infra_problems.append(f"{module} {version}: release API HTTP {e.code}")
        continue
    except Exception as e:
        infra_problems.append(f"{module} {version}: release API error: {e}")
        continue

    exp_version, exp = pick_expectations(module, version)
    if exp is None:
        # Older than every mapped version: this release predates the
        # expectations regime. Still run the structural checks below, but be
        # loud that no content markers were asserted (see the map's _doc).
        log(f"NOTE {module}@{version}: predates every mapped version — structural checks only")
        exp = {}
        exp_note = "predates expectations map, structural checks only"
    elif exp_version != version:
        log(f"NOTE {module}@{version}: no exact map entry — falling back to {exp_version}'s markers")
        exp_note = f"markers from {exp_version} (fallback)"
    else:
        exp_note = None

    qml_markers = exp.get("qml_grep", [])
    bin_markers = exp.get("binary_strings", [])
    ancestors = exp.get("required_ancestors", [])

    # Download the .lgx (ONE archive carrying all variants) + extract it.
    assets = [a for a in release.get("assets", []) if a.get("name", "").endswith(".lgx")]
    if len(assets) != 1:
        problems.append(f"{module} {version}: expected exactly one .lgx asset on {tag}, "
                        f"found {[a.get('name') for a in release.get('assets', [])]}")
        continue
    asset = assets[0]
    with tempfile.TemporaryDirectory(prefix="canary-relcontent-") as tmp:
        lgx = os.path.join(tmp, asset["name"])
        try:
            with fetch(asset["browser_download_url"], timeout=300) as r, open(lgx, "wb") as f:
                shutil.copyfileobj(r, f)
        except Exception as e:
            infra_problems.append(f"{module} {version}: could not download {asset['name']}: {e}")
            continue
        log(f"downloaded {asset['name']} ({os.path.getsize(lgx)//1024} KiB)")
        dest = os.path.join(tmp, "x")
        try:
            with tarfile.open(lgx, "r:gz") as tf:   # .lgx = gzipped tar
                try:
                    tf.extractall(dest, filter="data")  # py>=3.12 safe-extract
                except TypeError:
                    tf.extractall(dest)
        except Exception as e:
            problems.append(f"{module} {version}: {asset['name']} is not a valid gzipped tar: {e}")
            continue

        # Structural: top-level manifest must agree with the module + version.
        ok_manifest = False
        try:
            with open(os.path.join(dest, "manifest.json")) as f:
                man = json.load(f)
            ok_manifest = man.get("name") == module and man.get("version") == version
            if not ok_manifest:
                problems.append(f"{module} {version}: manifest.json says "
                                f"{man.get('name')}@{man.get('version')}")
        except Exception as e:
            problems.append(f"{module} {version}: no readable manifest.json in .lgx: {e}")
        log(f"{'PASS' if ok_manifest else 'FAIL'} {module}@{version} manifest.json name/version")

        # Per-variant marker assertions.
        misses = 0
        for variant in VARIANTS:
            vdir = os.path.join(dest, "variants", variant)
            if not os.path.isdir(vdir):
                log(f"FAIL {module}@{version} {variant}: variant missing from .lgx")
                problems.append(f"{module} {version}: variant {variant} missing from .lgx")
                misses += 1
                continue
            log(f"PASS {module}@{version} {variant}: variant present")
            for markers, native_only, kind in ((qml_markers, False, "qml_grep"),
                                               (bin_markers, True, "binary_strings")):
                if not markers:
                    continue
                found = scan_variant(vdir, markers, native_only)
                for m in markers:
                    ok = m in found
                    log(f"{'PASS' if ok else 'FAIL'} {module}@{version} {variant} {kind} {m!r}")
                    if not ok:
                        problems.append(f"{module} {version} {variant}: {kind} marker {m!r} "
                                        f"not found in released artifact")
                        misses += 1

    # Ancestry: the tag's target commit must contain the required commits.
    anc_ok = 0
    if ancestors:
        try:
            target = tag_target_commit(tag)
        except Exception as e:
            infra_problems.append(f"{module} {version}: could not resolve tag {tag} target: {e}")
            target = None
        if target:
            log(f"tag {tag} -> commit {target}")
            for anc in ancestors:
                try:
                    ok, how = is_ancestor(anc, target)
                except Exception as e:
                    infra_problems.append(
                        f"{module} {version}: ancestry of {anc} undecidable: {e}")
                    continue
                log(f"{'PASS' if ok else 'FAIL'} {module}@{version} required_ancestor "
                    f"{anc} (via {how})")
                if ok:
                    anc_ok += 1
                else:
                    problems.append(f"{module} {version}: required ancestor {anc} is NOT an "
                                    f"ancestor of tag {tag} target {target} — release cut "
                                    f"from a stale ref")
                    misses += 1

    summary = (f"{module}@{version}: {len(VARIANTS)} variants x "
               f"{len(qml_markers) + len(bin_markers)} markers, "
               f"{anc_ok}/{len(ancestors)} ancestors ok")
    if exp_note:
        summary += f" [{exp_note}]"
    if misses:
        summary += f" — {misses} MISS(ES)"
    checked.append(summary)

# Content-level defects win over infra noise (same policy as leg-catalog.sh).
if problems:
    die("fail", "; ".join(problems))
if infra_problems:
    die("broken", "could not fully verify release content: " + "; ".join(infra_problems))
if not checked:
    die("broken", "nothing was verified (no modules resolved)")
die("pass", "release content ok: " + "; ".join(checked))
PY
)"

STATUS="${OUT%%|*}"
EVIDENCE="${OUT#*|}"
[ -z "$STATUS" ] && { STATUS=broken; EVIDENCE="release-content validator produced no output"; }
emit_result "$LEG" "$STATUS" "$EVIDENCE"
exit $?
