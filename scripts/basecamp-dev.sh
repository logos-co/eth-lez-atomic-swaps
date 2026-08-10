#!/usr/bin/env bash
set -euo pipefail

# basecamp-dev.sh — LOCAL working-tree module harness for real Basecamp on macOS.
#
# WHAT THIS IS
# ------------
# The fastest inner loop for "did my C++/QML change actually load in Basecamp?":
# it builds swap (core) + swap_ui (ui plugin) from the CURRENT WORKING TREE,
# installs them plus the pinned delivery_module dependency into an isolated dev
# profile using scaffold's own pinned `lgpm`, stamps a visibly distinct
# `-dev.<sha>` version into each staged manifest, and launches Basecamp against
# that profile. ~3-6 min warm, no CI, no catalogue, no module-manager service,
# no release.
#
# RELATIONSHIP TO SCAFFOLD (`lgs`)
# --------------------------------
# This is a THIN composition over what scaffold already provides, filling the
# two gaps scaffold's `lgs basecamp` verbs structurally cannot cover for a
# working-tree dev loop:
#
#   REUSED FROM SCAFFOLD:
#     * scaffold.toml is the single source of truth for the delivery_module,
#       lgpm, and basecamp pins — this script reads them, hardcodes nothing.
#     * The pinned `lgpm cli-portable` (scaffold's install tool) performs the
#       install, so staged manifests round-trip `view` + `hashes` exactly like
#       `make basecamp-ui-smoke` and `lgs basecamp install`.
#     * The pinned basecamp bundle (scaffold's basecamp pin) is the preferred
#       launch target when it is already in the Nix store.
#     * The Makefile's layout-drift guard pattern is reused before launch.
#
#   GAPS THIS HARNESS FILLS (candidate scaffold feature requests):
#     1. WORKING-TREE BUILDS. `lgs basecamp build/install` build the *captured*
#        (committed) source set — `git+file:.?dir=…` refs in scaffold.toml. A
#        dev loop needs the LIVE working tree, so this script builds the path
#        flake refs `./swap-module#lgx-portable` / `./swap-ui#lgx-portable`.
#     2. DEV-VERSION STAMP. Nothing in `lgs` stamps an installed module so the
#        corner badge proves which build is live. This script does.
#     3. NO-SCRUB LAUNCH. `lgs basecamp launch` scrubs the profile and REPLAYS
#        the captured module set on every invocation — which would wipe the
#        stamp. This script launches Basecamp directly against the already
#        staged, stamped profile.
#
# WHY THE `-dev.<sha>` STAMP MATTERS
# ----------------------------------
# Three times the owner's Basecamp kept LOADING a stale module after a catalogue
# update, with no way to tell from the GUI which build was live. The stamped
# version is the fix: the corner version badge reads the exact staged version,
# so "which build am I running?" is answered by looking, not guessing.
#
# See docs/local-dev-harness.md for when to use this vs the canary channel vs
# the official catalogue, and for what this harness does NOT exercise.

usage() {
  cat <<'EOF'
Usage: scripts/basecamp-dev.sh [options]

Options:
  --skip-build        Reuse the last-built swap / swap_ui LGX artifacts (no nix build).
  --no-launch         Stage + verify, print the exact launch command, and stop.
  --pinned-basecamp   Force building/using scaffold's pinned basecamp bundle
                      (reproducible; may trigger a long first-time build on macOS).
  --installed-app     Force using the installed /Applications Basecamp .app.
  -h, --help          Show this help.

Environment overrides:
  DEV_ROOT              Dev root / isolated user-dir (default: ~/.eth-lez-dev).
  BASECAMP_APP          Path to a Basecamp .app to launch (skips app resolution).
  DELIVERY_MODULE_LGX   Path to a delivery_module .lgx to stage instead of building it.

Two-command owner workflow:
  make basecamp-dev              # build working tree + stage + launch
  # ...edit swap-module/ or swap-ui/, then:
  make basecamp-dev              # again — badge shows the new -dev.<sha>
EOF
}

SKIP_BUILD=0
NO_LAUNCH=0
FORCE_PINNED=0
FORCE_INSTALLED=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=1 ;;
    --no-launch) NO_LAUNCH=1 ;;
    --pinned-basecamp) FORCE_PINNED=1 ;;
    --installed-app) FORCE_INSTALLED=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

# ---------------------------------------------------------------------------
# Repo + tooling.
# ---------------------------------------------------------------------------
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo_root"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "basecamp-dev.sh targets macOS (found $(uname -s)). On Linux use 'make basecamp-ui-smoke'." >&2
  exit 1
fi
for command_name in nix git tar file python3; do
  command -v "$command_name" >/dev/null 2>&1 || { echo "missing required command: $command_name" >&2; exit 1; }
done

DEV_ROOT=${DEV_ROOT:-"$HOME/.eth-lez-dev"}
# Basecamp derives its whole data tree — modules/, plugins/, module_data/, logs/
# — from LOGOS_USER_DIR / --user-dir (app/utils/LogosBasecampPaths.h
# baseDirectory; docs/scaffold-upstream-tracker.md TR-21). So the staged modules
# MUST live under the user-dir: user-dir == the isolated dev profile. This
# mirrors the CI smoke's throwaway user-dir and scaffold's per-profile layout.
user_dir="$DEV_ROOT/profile"
modules_dir="$user_dir/modules"
plugins_dir="$user_dir/plugins"
cache_dir="$DEV_ROOT/cache"
mkdir -p "$cache_dir"

# scaffold.toml is the single source of truth for every pin.
toml_get() {
  python3 - "$repo_root/scaffold.toml" "$1" "$2" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
section, key = sys.argv[2], sys.argv[3]
m = re.search(r'\[' + re.escape(section) + r'\](.*?)(?:\n\[|\Z)', text, re.S)
if not m:
    sys.exit(0)
km = re.search(r'(?m)^\s*' + re.escape(key) + r'\s*=\s*"([^"]+)"', m.group(1))
print(km.group(1) if km else "")
PY
}
lgpm_source=$(toml_get repos.lgpm source)
lgpm_pin=$(toml_get repos.lgpm pin)
basecamp_source=$(toml_get repos.basecamp source)
basecamp_pin=$(toml_get repos.basecamp pin)
# [repos.basecamp].attr is an inline per-system table
# ({ aarch64-darwin = "bin-macos-app", ... }); pull the arm64-darwin entry.
basecamp_attr=$(python3 - "$repo_root/scaffold.toml" <<'PY'
import re, sys
t = open(sys.argv[1]).read()
m = re.search(r'\[repos\.basecamp\](.*?)(?:\n\[|\Z)', t, re.S)
a = re.search(r'aarch64-darwin\s*=\s*"([^"]+)"', m.group(1)) if m else None
print(a.group(1) if a else "")
PY
)
basecamp_attr=${basecamp_attr:-bin-macos-app}
delivery_flake=$(toml_get modules.delivery_module flake)
[[ -n "$lgpm_pin" && -n "$basecamp_pin" ]] || { echo "could not read lgpm/basecamp pins from scaffold.toml" >&2; exit 1; }
basecamp_ref="github:logos-co/logos-basecamp/$basecamp_pin#$basecamp_attr"
lgpm_ref="$lgpm_source/$lgpm_pin#cli-portable"

# ---------------------------------------------------------------------------
# Dev version suffix: proves, at a glance, which build is live.
# ---------------------------------------------------------------------------
short_sha=$(git rev-parse --short HEAD)
dirty=""
if ! git diff --quiet || ! git diff --cached --quiet; then
  dirty=".dirty"
fi
dev_suffix="dev.${short_sha}${dirty}"

echo "==> eth-lez-atomic-swaps local Basecamp dev harness"
echo "    repo root   : $repo_root"
echo "    dev root    : $DEV_ROOT"
echo "    user-dir    : $user_dir"
echo "    dev suffix  : -$dev_suffix"
echo "    scaffold pins: lgpm@${lgpm_pin:0:12} basecamp@${basecamp_pin:0:12} ($basecamp_attr)"
echo

# ---------------------------------------------------------------------------
# nix build helpers.
# ---------------------------------------------------------------------------
build_out() {
  # Realize a flake ref; print its single out-path on stdout, progress on stderr.
  local label=$1 ref=$2; shift 2
  echo "==> building $label ($ref)" >&2
  local start out tmp="/tmp/basecamp-dev.$$.$RANDOM"
  start=$(date +%s)
  if ! nix build "$ref" --no-link --print-out-paths "$@" >"$tmp" 2>"$tmp.err"; then
    echo "$label build failed:" >&2; cat "$tmp.err" >&2; rm -f "$tmp" "$tmp.err"; return 1
  fi
  out=$(sed '/^$/d' "$tmp"); rm -f "$tmp" "$tmp.err"
  echo "    $label built in $(( $(date +%s) - start ))s -> $out" >&2
  printf '%s\n' "$out"
}
find_single_lgx() {
  local label=$1 dir=$2 matches=()
  while IFS= read -r c; do matches+=("$c"); done < <(find "$dir" -maxdepth 1 -type f -name '*.lgx' -print)
  [[ ${#matches[@]} -eq 1 ]] || { echo "$label: expected one .lgx in $dir, found ${#matches[@]}" >&2; return 1; }
  printf '%s\n' "${matches[0]}"
}

# ---------------------------------------------------------------------------
# 1. Pinned lgpm (scaffold's install tool) — cached across runs.
# ---------------------------------------------------------------------------
lgpm_out=$(build_out "lgpm (scaffold pin)" "$lgpm_ref")
lgpm_bin="$lgpm_out/bin/lgpm"
[[ -x "$lgpm_bin" ]] || { echo "pinned lgpm binary missing: $lgpm_bin" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 2. Working-tree module builds (GAP #1: path refs = live working tree).
# ---------------------------------------------------------------------------
swap_path_file="$cache_dir/last-swap.out"
swap_ui_path_file="$cache_dir/last-swap_ui.out"
if [[ "$SKIP_BUILD" == 1 ]]; then
  echo "==> --skip-build: reusing last-built artifacts"
  [[ -s "$swap_path_file" && -s "$swap_ui_path_file" ]] || { echo "no cached build; run once without --skip-build" >&2; exit 1; }
  swap_out=$(cat "$swap_path_file"); swap_ui_out=$(cat "$swap_ui_path_file")
  [[ -d "$swap_out" && -d "$swap_ui_out" ]] || { echo "cached out-paths gone from store; rebuild without --skip-build" >&2; exit 1; }
else
  build_start=$(date +%s)
  swap_out=$(build_out swap './swap-module#lgx-portable')
  swap_ui_out=$(build_out swap_ui './swap-ui#lgx-portable')
  echo "$swap_out" > "$swap_path_file"; echo "$swap_ui_out" > "$swap_ui_path_file"
  echo "==> working-tree module builds finished in $(( $(date +%s) - build_start ))s"
fi
swap_lgx=$(find_single_lgx swap "$swap_out")
swap_ui_lgx=$(find_single_lgx swap_ui "$swap_ui_out")

# ---------------------------------------------------------------------------
# 3. delivery_module dependency: override | cache | build the scaffold pin.
# ---------------------------------------------------------------------------
delivery_cache="$cache_dir/delivery_module.lgx"
if [[ -n "${DELIVERY_MODULE_LGX:-}" ]]; then
  [[ -f "$DELIVERY_MODULE_LGX" ]] || { echo "DELIVERY_MODULE_LGX not found: $DELIVERY_MODULE_LGX" >&2; exit 1; }
  echo "==> delivery_module: using override $DELIVERY_MODULE_LGX"; delivery_lgx="$DELIVERY_MODULE_LGX"
elif [[ -f "$delivery_cache" ]]; then
  echo "==> delivery_module: reusing cached $delivery_cache"; delivery_lgx="$delivery_cache"
else
  [[ "$delivery_flake" == *"#lgx" ]] || { echo "bad [modules.delivery_module].flake in scaffold.toml: $delivery_flake" >&2; exit 1; }
  # The standalone delivery_module flake pins its OWN nixpkgs, which predates
  # crates.io's User-Agent fix, so its zerokit vendor step dies with a 403 (the
  # working-tree swap/swap_ui flakes don't hit this — they pin nixpkgs-crates-io
  # internally). Mirror the CI smoke's single scoped override exactly. Drop this
  # once logos-delivery advances its nixpkgs past 2026-04-26.
  delivery_nixpkgs='github:danisharora099/nixpkgs/3134d7bb12629545b1f3e5b1d2faadbf861484fd'
  delivery_out=$(build_out delivery_module "${delivery_flake%#lgx}#lgx-portable" \
    --override-input logos-delivery/nixpkgs "$delivery_nixpkgs")
  cp "$(find_single_lgx delivery_module "$delivery_out")" "$delivery_cache"
  echo "    cached delivery_module -> $delivery_cache"; delivery_lgx="$delivery_cache"
fi

# ---------------------------------------------------------------------------
# 4. Install via pinned lgpm (REUSED: same install path as the CI smoke and
#    `lgs basecamp install` — auto-classifies core -> modules/, ui_qml ->
#    plugins/, and round-trips view + hashes into each manifest).
# ---------------------------------------------------------------------------
echo
echo "==> installing into $user_dir via pinned lgpm"
rm -rf "$modules_dir" "$plugins_dir"
mkdir -p "$modules_dir" "$plugins_dir"
for lgx in "$delivery_lgx" "$swap_lgx" "$swap_ui_lgx"; do
  "$lgpm_bin" --modules-dir "$modules_dir" --ui-plugins-dir "$plugins_dir" install --file "$lgx"
done

# ---------------------------------------------------------------------------
# 5. Dev-version stamp (GAP #2). delivery_module is the pinned dependency, not a
#    working-tree build, so it keeps its released version.
# ---------------------------------------------------------------------------
echo "==> stamping dev version into working-tree modules"
stamp_manifest() {
  python3 - "$1" "$dev_suffix" <<'PY'
import json, sys
path, suffix = sys.argv[1], sys.argv[2]
d = json.load(open(path))
base = d.get("version", "0.0.0").split("-dev.", 1)[0]   # idempotent re-stamp
d["version"] = f"{base}-{suffix}"
json.dump(d, open(path, "w"), indent=2, ensure_ascii=False); open(path, "a").write("\n")
print(f"    {d['name']}: version -> {d['version']}")
PY
}
stamp_manifest "$modules_dir/swap/manifest.json"
stamp_manifest "$plugins_dir/swap_ui/manifest.json"

# ---------------------------------------------------------------------------
# 6. Self-verification (file-level; GUI stays with the human).
# ---------------------------------------------------------------------------
echo
echo "==> verifying staged dev root"
fail=0
note() { echo "    [ ok ] $1"; }
bad()  { echo "    [FAIL] $1" >&2; fail=1; }
for path in "$modules_dir/delivery_module/manifest.json" "$modules_dir/swap/manifest.json" "$plugins_dir/swap_ui/manifest.json"; do
  [[ -f "$path" ]] && note "manifest present: ${path#$user_dir/}" || bad "missing manifest: $path"
done
python3 - "$modules_dir/swap/manifest.json" "$plugins_dir/swap_ui/manifest.json" "$dev_suffix" <<'PY' || fail=1
import json, sys
s = json.load(open(sys.argv[1])); u = json.load(open(sys.argv[2])); suffix = sys.argv[3]
ok = True
for label, m in (("swap", s), ("swap_ui", u)):
    if not m.get("version", "").endswith(suffix):
        print(f"    [FAIL] {label} version not stamped: {m.get('version')}"); ok = False
    else:
        print(f"    [ ok ] {label} stamped: {m['version']}")
if "delivery_module" not in (s.get("dependencies") or []):
    print("    [FAIL] swap lost its delivery_module dependency"); ok = False
if u.get("type") != "ui_qml" or not u.get("view"):
    print(f"    [FAIL] swap_ui not a ui_qml plugin with a view: type={u.get('type')} view={u.get('view')}"); ok = False
else:
    print(f"    [ ok ] swap_ui view preserved: {u['view']}")
sys.exit(0 if ok else 1)
PY
dylib_count=0
while IFS= read -r dylib; do
  dylib_count=$((dylib_count + 1))
  if file "$dylib" | grep -q 'arm64'; then note "arm64 dylib: ${dylib#$user_dir/}"
  else bad "non-arm64 dylib: $dylib ($(file -b "$dylib"))"; fi
done < <(find "$modules_dir/swap" "$plugins_dir/swap_ui" -type f \( -name '*.dylib' -o -name '*.so' \))
[[ "$dylib_count" -gt 0 ]] || bad "no dylib/.so found under staged swap / swap_ui"
[[ "$fail" == 0 ]] || { echo "==> staging verification FAILED — not launching" >&2; exit 1; }
echo "==> staging verification passed"

# ---------------------------------------------------------------------------
# 7. Resolve the Basecamp bundle to launch.
#    Delivery context creation (delivery_module.createNode) is validated ONLY
#    against the scaffold-pinned Basecamp. On an older host (e.g. 0.2.1) it
#    fails at startup with "createNode failed: Failed to create Delivery
#    context" — see the root-cause note in docs/local-dev-harness.md. So the
#    harness now PROVIDES the validated bundle itself, in strict order:
#      1. cached pinned bundle  — a prior run's gc-rooted nix out-link; instant.
#      2. build pinned bundle   — one-time cold macOS build (~5-15 min), then
#                                 cached under $cache_dir for instant reuse.
#      3. installed /Applications app — LAST RESORT. Used directly (no build)
#         only when it is already >= the validated version; otherwise it is
#         launched with a LOUD banner because delivery is known to break there.
# ---------------------------------------------------------------------------

# The scaffold basecamp pin ([repos.basecamp].pin) builds this Basecamp release;
# it is the version delivery_module.createNode is validated against. Bump in
# lockstep when the pin advances to a new Basecamp release.
validated_basecamp_version="0.2.3"
pinned_link="$cache_dir/basecamp-pinned"   # nix out-link == gc-rooted cache

plist_version() {
  /usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" \
    "$1/Contents/Info.plist" 2>/dev/null || echo "?"
}
# Exit 0 iff $1 >= $2 as dotted-numeric versions; non-zero on "older" OR when
# either side is unparseable (so an unknown version never masquerades as new).
version_ge() {
  python3 - "$1" "$2" <<'PY'
import sys
def parse(v):
    parts = [int(p) for p in v.split(".") if p.isdigit()]
    return tuple(parts) if parts else None
a, b = parse(sys.argv[1]), parse(sys.argv[2])
sys.exit(0 if (a is not None and b is not None and a >= b) else 1)
PY
}
app_in_bundle() { find "$1" -maxdepth 2 -iname '*basecamp*.app' -print 2>/dev/null | head -1; }
bin_in_app()    { local b="$1/Contents/MacOS/LogosBasecamp"; [[ -x "$b" ]] && printf '%s\n' "$b"; }
find_installed_app() { find /Applications -maxdepth 1 -iname '*basecamp*.app' -print 2>/dev/null | head -1; }

resolve_basecamp_bin() {
  local out app bin
  # Explicit override wins, unconditionally.
  if [[ -n "${BASECAMP_APP:-}" ]]; then
    bin=$(bin_in_app "$BASECAMP_APP") \
      || { echo "BASECAMP_APP has no LogosBasecamp binary: $BASECAMP_APP/Contents/MacOS/LogosBasecamp" >&2; return 1; }
    echo "installed(override):$BASECAMP_APP:$bin"; return 0
  fi

  if [[ "$FORCE_INSTALLED" == 1 ]]; then
    app=$(find_installed_app)
    [[ -n "$app" ]] || { echo "no Basecamp .app under /Applications; set BASECAMP_APP or drop --installed-app" >&2; return 1; }
    bin=$(bin_in_app "$app") || { echo "installed app has no LogosBasecamp binary: $app" >&2; return 1; }
    echo "installed:$app:$bin"; return 0
  fi

  # (1) Cached pinned bundle — a prior run's out-link, still valid in the store.
  if [[ "$FORCE_PINNED" == 0 && -L "$pinned_link" ]]; then
    out=$(readlink "$pinned_link" 2>/dev/null || true)
    if [[ -n "$out" && -d "$out" ]]; then
      app=$(app_in_bundle "$out"); bin=$(bin_in_app "$app" 2>/dev/null || true)
      [[ -n "$bin" ]] && { echo "pinned-cached:$app:$bin"; return 0; }
    fi
  fi

  # An installed app already >= the validated version is a valid host — use it
  # and skip the long build. (Not applicable under --pinned-basecamp.)
  if [[ "$FORCE_PINNED" == 0 ]]; then
    app=$(find_installed_app)
    if [[ -n "$app" ]] && version_ge "$(plist_version "$app")" "$validated_basecamp_version"; then
      bin=$(bin_in_app "$app") && { echo "installed-ok:$app:$bin"; return 0; }
    fi
  fi

  # (2) Build the pinned bundle, caching it via a gc-rooted out-link so the next
  #     run hits tier (1) instantly. nix progress streams to the terminal
  #     (stderr); 1>&2 keeps nix's stdout out of the captured result.
  echo "==> Basecamp: building scaffold's pinned bundle v$validated_basecamp_version ($basecamp_attr)." >&2
  echo "    ONE-TIME cost (cold: ~5-15 min on macOS; then cached to" >&2
  echo "    $pinned_link for instant reuse). Delivery context creation is" >&2
  echo "    validated only on this bundle. Skip with --installed-app / BASECAMP_APP." >&2
  local start; start=$(date +%s)
  if nix build "$basecamp_ref" --out-link "$pinned_link" 1>&2; then
    out=$(readlink "$pinned_link" 2>/dev/null || true)
    app=$(app_in_bundle "$out"); bin=$(bin_in_app "$app" 2>/dev/null || true)
    if [[ -n "$bin" ]]; then
      echo "    pinned bundle ready in $(( $(date +%s) - start ))s -> $app" >&2
      echo "pinned-built:$app:$bin"; return 0
    fi
    echo "pinned basecamp build produced no LogosBasecamp binary under $out" >&2
  else
    echo "pinned basecamp build FAILED (see nix output above)." >&2
  fi

  # --pinned-basecamp is an explicit reproducibility request: never silently
  # downgrade to the installed app.
  [[ "$FORCE_PINNED" == 1 ]] && { echo "could not provide the pinned Basecamp bundle (see error above)" >&2; return 1; }

  # (3) LAST RESORT: installed app (a loud banner fires below if it is stale).
  app=$(find_installed_app)
  [[ -n "$app" ]] || { echo "no pinned bundle and no Basecamp .app under /Applications; set BASECAMP_APP or --pinned-basecamp" >&2; return 1; }
  bin=$(bin_in_app "$app") || { echo "installed app has no LogosBasecamp binary: $app" >&2; return 1; }
  echo "installed-fallback:$app:$bin"; return 0
}

resolved=$(resolve_basecamp_bin) || exit 1
bc_kind=${resolved%%:*}; rest=${resolved#*:}; bc_app=${rest%%:*}; basecamp_bin=${rest#*:}
app_version=$(plist_version "$bc_app")

# Loud banner when we could only reach an installed host older than the
# validated version — delivery context creation is known to break there.
case "$bc_kind" in
  installed|installed-fallback|"installed(override)")
    if ! version_ge "$app_version" "$validated_basecamp_version"; then
      cat >&2 <<BANNER

  ##########################################################################
  ##  WARNING  host Basecamp v$app_version  <  validated v$validated_basecamp_version
  ##  This host is KNOWN TO BREAK delivery context creation
  ##  ("createNode failed: Failed to create Delivery context"): swap offers
  ##  will NOT load and any delivery-dependent result is UNRELIABLE.
  ##  Fix: let the harness build the pinned bundle (drop --installed-app /
  ##  BASECAMP_APP), or install Basecamp >= v$validated_basecamp_version.
  ##########################################################################
BANNER
    fi
    ;;
esac

# ---------------------------------------------------------------------------
# 8. Layout-drift guard (REUSED pattern from Makefile basecamp-launch-%): the
#    dirs we hand Basecamp must be the ones we actually staged into.
# ---------------------------------------------------------------------------
[[ -d "$modules_dir/swap" && -d "$plugins_dir/swap_ui" ]] || {
  echo "layout drift: staged swap/swap_ui not found under $user_dir" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 9. Echo exactly what will load, then launch (or print the command).
# ---------------------------------------------------------------------------
mver() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$1"; }
staged_swap_ver=$(mver "$modules_dir/swap/manifest.json")
staged_ui_ver=$(mver "$plugins_dir/swap_ui/manifest.json")
staged_delivery_ver=$(mver "$modules_dir/delivery_module/manifest.json")

echo
echo "==> ready"
echo "    Basecamp          : $bc_app"
echo "    Basecamp version  : v$app_version  (validated: v$validated_basecamp_version) [$bc_kind]"
echo "    isolated user-dir : $user_dir"
echo "    modules dir       : $modules_dir"
echo "    plugins dir       : $plugins_dir"
echo "    loaded modules    : swap $staged_swap_ver | swap_ui $staged_ui_ver | delivery_module $staged_delivery_ver"
echo
echo "    >>> corner version badge should read: $staged_ui_ver <<<"
echo

# Belt-and-suspenders: the load path is --user-dir (Basecamp exports it as
# LOGOS_USER_DIR and reads modules/ + plugins/ beneath it). The explicit dir
# exports document intent and cover any pin that consults them directly; they
# point at the same staged tree.
launch_cmd=(
  env
  LOGOS_USER_DIR="$user_dir"
  LOGOS_MODULES_DIR="$modules_dir"
  LOGOS_UI_PLUGINS_DIR="$plugins_dir"
  "$basecamp_bin" --user-dir "$user_dir"
)

kill_prior_dev_instance() {
  local pids
  pids=$(pgrep -f "LogosBasecamp.*$user_dir" 2>/dev/null || true)
  if [[ -n "$pids" ]]; then
    echo "==> killing prior dev-profile Basecamp: $pids"
    # shellcheck disable=SC2086
    kill $pids 2>/dev/null || true; sleep 1
    pids=$(pgrep -f "LogosBasecamp.*$user_dir" 2>/dev/null || true)
    # shellcheck disable=SC2086
    [[ -n "$pids" ]] && kill -9 $pids 2>/dev/null || true
  fi
}
print_launch_cmd() { echo "==> launch command:"; printf '   '; printf ' %q' "${launch_cmd[@]}"; printf '\n'; }

if [[ "$NO_LAUNCH" == 1 ]]; then
  print_launch_cmd
  echo; echo "==> --no-launch: staged and verified; launch skipped."
  exit 0
fi

kill_prior_dev_instance

# GUI verification is deliberately the human's job — the entire point of the
# `-dev.<sha>` stamp is that the corner badge answers "which build is live?"
# at a glance. The script's own verification stops at the file level above.
print_launch_cmd
echo "==> launching Basecamp — check the corner badge reads: $staged_ui_ver"
exec "${launch_cmd[@]}"
