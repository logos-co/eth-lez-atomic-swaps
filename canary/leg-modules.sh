#!/usr/bin/env bash
# leg-modules.sh — proves both Basecamp modules still build to a portable .lgx
# from source, the exact artifact the release pipeline ships and the catalog
# indexes. Runs `nix build .#lgx-portable` in each module dir.
#
#   PASS  = both modules build; the out-links resolve to a .lgx.
#   FAIL  = a module's nix build failed (a real regression in the module or its
#           pinned toolchain — the thing this leg exists to catch).
#   BROKEN= nix missing, or invoked on a non-darwin-arm64 host (only that
#           platform has real pinned hashes today — see swap-module/flake.nix
#           and issue #32; other systems fall back to fakeSha256 and cannot build).
#
# darwin-arm64 only. Nix caches aggressively, so a warm run is fast; a cold run
# compiles the C++/Rust module + rapidsnark and can take many minutes.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=canary/lib/common.sh
source "$HERE/lib/common.sh"
ROOT="$(canary_repo_root)"
LEG="modules"

if ! command -v nix >/dev/null 2>&1; then
  emit_result "$LEG" broken "nix not found on PATH" ; exit $?
fi

ARCH="$(uname -m)"
OS="$(uname -s)"
if [ "$OS" != "Darwin" ] || [ "$ARCH" != "arm64" ]; then
  emit_result "$LEG" broken \
    "module .lgx-portable builds are darwin-arm64-only today (host is $OS/$ARCH; only aarch64-darwin has real pinned hashes — swap-module/flake.nix, issue #32)"
  exit $?
fi

declare -a MODULES=("swap-module" "swap-ui")
built=()
for m in "${MODULES[@]}"; do
  dir="$ROOT/$m"
  out="$(mktemp -u "/tmp/canary-out-$m.XXXXXX")"   # out-link in /tmp, never the repo
  canary_log "nix build $m #lgx-portable ..."
  if ! ( cd "$dir" && nix build ".#lgx-portable" \
            --out-link "$out" \
            --print-build-logs 2>&1 | tail -8 ); then
    rm -f "$out"
    emit_result "$LEG" fail "nix build .#lgx-portable failed for $m (see build log above)"
    exit $?
  fi
  # Resolve the out-link and confirm it yields a .lgx artifact.
  real="$(readlink "$out" 2>/dev/null || true)"
  lgx="$(find -L "$out" -name '*.lgx' 2>/dev/null | head -1)"
  rm -f "$out"
  if [ -z "$lgx" ]; then
    emit_result "$LEG" fail "$m built but produced no .lgx under $real"
    exit $?
  fi
  built+=("$m($(basename "$lgx"))")
done

emit_result "$LEG" pass "nix .#lgx-portable ok on $OS/$ARCH: ${built[*]}"
exit $?
