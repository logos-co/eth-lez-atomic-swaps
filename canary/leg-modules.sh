#!/usr/bin/env bash
# leg-modules.sh — proves both Basecamp modules still build to a portable .lgx
# from source, the exact artifact the release pipeline ships and the catalog
# indexes. Runs `nix build .#lgx-portable` in each module dir.
#
#   PASS  = both modules build; the out-links resolve to a .lgx.
#   FAIL  = a module's nix build failed (a real regression in the module or its
#           pinned toolchain — the thing this leg exists to catch).
#   BROKEN= nix missing, or invoked on a host/arch combo with no pinned hash
#           in swap-module/flake.nix (see below).
#
# Issue #32 (PR #53) pinned real circuits/rapidsnark hashes for darwin-arm64,
# linux-amd64 (x86_64-linux), and linux-arm64 (aarch64-linux) — those are the
# three variants swap-module/flake.nix can actually build today (no
# x86_64-darwin: upstream ships no macos-x86_64 circuits bundle). Nix caches
# aggressively, so a warm run is fast; a cold run compiles the C++/Rust
# module + rapidsnark and can take many minutes.
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
case "$OS/$ARCH" in
  Darwin/arm64|Linux/x86_64|Linux/aarch64) ;;
  *)
    emit_result "$LEG" broken \
      "module .lgx-portable builds have no pinned hash for $OS/$ARCH — swap-module/flake.nix only pins darwin-arm64, linux-amd64 (x86_64-linux), and linux-arm64 (aarch64-linux) (issue #32 / PR #53)"
    exit $?
    ;;
esac

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
