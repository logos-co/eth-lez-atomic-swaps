#!/usr/bin/env bash
# canary/lib/localnet.sh — boot / tear down a THROWAWAY LEZ localnet for the
# chain leg, without disturbing any developer's `.scaffold` state.
#
# Strategy (fast path): run a prebuilt `sequencer_service` binary directly
# against the checked-in debug config (genesis-funded accounts, RISC0_DEV_MODE),
# writing all state to a fresh /tmp dir on a free port. This is exactly how the
# canary's local proof was produced — no `logos-scaffold setup` rebuild needed
# if a prebuilt binary for the pinned LEZ commit is already on disk.
#
# It finds a prebuilt binary via, in order:
#   1. $CANARY_SEQ_BIN (explicit override)
#   2. any sibling scaffold cache: */.scaffold/lez-cache/repos/lez/<pin>/target/release/sequencer_service
#   3. this repo's own .scaffold cache (after `make setup`)
# If none is found it falls back to `logos-scaffold localnet start` (canonical,
# but requires the full toolchain built — slow on a cold machine / CI).
#
# Exports on success: CANARY_LOCALNET_RPC (e.g. http://127.0.0.1:3055)
set -uo pipefail

CANARY_LOCALNET_PID=""
CANARY_LOCALNET_HOME=""
CANARY_LOCALNET_MODE=""

_canary_lez_pin() {
  local root; root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  sed -n '/^\[repos\.lez\]/,/^\[/p' "$root/scaffold.toml" \
    | sed -n 's/^pin = "\([0-9a-f]*\)".*/\1/p' | head -1
}

_canary_find_seq_bin() {
  local pin="$1"
  if [ -n "${CANARY_SEQ_BIN:-}" ] && [ -x "$CANARY_SEQ_BIN" ]; then
    echo "$CANARY_SEQ_BIN"; return 0
  fi
  # sibling conductor workspaces + this repo
  local hit
  hit="$(ls -1 \
    "$HOME"/conductor/workspaces/*/*/.scaffold/lez-cache/repos/lez/"$pin"/target/release/sequencer_service \
    ./.scaffold/lez-cache/repos/lez/"$pin"/target/release/sequencer_service \
    2>/dev/null | head -1)"
  [ -n "$hit" ] && { echo "$hit"; return 0; }
  return 1
}

_canary_free_port() {
  local p
  for p in 3055 3056 3057 3058 3059 3060; do
    if ! lsof -iTCP:"$p" -sTCP:LISTEN -n -P >/dev/null 2>&1; then echo "$p"; return 0; fi
  done
  echo 3055
}

# Locate the checked-in debug sequencer config from the cargo git checkout of
# the pinned LEZ commit.
_canary_debug_config() {
  local pin_short="${1:0:7}"
  ls -1 "$HOME"/.cargo/git/checkouts/logos-execution-zone-*/"$pin_short"/lez/sequencer/service/configs/debug/sequencer_config.json 2>/dev/null | head -1
}

canary_localnet_start() {
  local pin bin cfg port home
  pin="$(_canary_lez_pin)"
  [ -z "$pin" ] && { echo "[localnet] could not read LEZ pin from scaffold.toml" >&2; return 1; }

  if bin="$(_canary_find_seq_bin "$pin")"; then
    cfg="$(_canary_debug_config "$pin")"
    if [ -z "$cfg" ]; then
      echo "[localnet] prebuilt binary found but no debug config checkout for $pin" >&2
      return 1
    fi
    port="$(_canary_free_port)"
    home="$(mktemp -d /tmp/canary-localnet.XXXXXX)"
    CANARY_LOCALNET_HOME="$home"
    python3 - "$cfg" "$home" > "$home/sequencer_config.json" <<'PY'
import json, sys
c = json.load(open(sys.argv[1])); c["home"] = sys.argv[2]
json.dump(c, sys.stdout, indent=2)
PY
    echo "[localnet] booting prebuilt sequencer ($bin) on :$port (home=$home)" >&2
    ( cd "$home" && RISC0_DEV_MODE=1 RUST_LOG=info \
        "$bin" "$home/sequencer_config.json" --port "$port" > "$home/seq.log" 2>&1 & echo $! > "$home/pid" )
    CANARY_LOCALNET_PID="$(cat "$home/pid")"
    CANARY_LOCALNET_MODE="prebuilt"
    # wait for readiness
    local i
    for i in $(seq 1 30); do
      if grep -q "RPC server started" "$home/seq.log" 2>/dev/null; then
        export CANARY_LOCALNET_RPC="http://127.0.0.1:$port"
        echo "[localnet] up at $CANARY_LOCALNET_RPC" >&2
        return 0
      fi
      if ! kill -0 "$CANARY_LOCALNET_PID" 2>/dev/null; then
        echo "[localnet] sequencer exited early; log:" >&2; tail -20 "$home/seq.log" >&2; return 1
      fi
      sleep 1
    done
    echo "[localnet] timed out waiting for sequencer readiness" >&2
    return 1
  fi

  # Fallback: canonical scaffold path (requires a full toolchain build).
  echo "[localnet] no prebuilt binary for $pin; falling back to 'logos-scaffold localnet start'" >&2
  if command -v logos-scaffold >/dev/null 2>&1; then
    logos-scaffold localnet start >&2 || return 1
    CANARY_LOCALNET_MODE="scaffold"
    export CANARY_LOCALNET_RPC="http://127.0.0.1:3040"
    return 0
  fi
  echo "[localnet] logos-scaffold not found either — cannot boot a localnet" >&2
  return 1
}

canary_localnet_stop() {
  case "$CANARY_LOCALNET_MODE" in
    prebuilt)
      [ -n "$CANARY_LOCALNET_PID" ] && kill "$CANARY_LOCALNET_PID" 2>/dev/null
      echo "[localnet] stopped prebuilt sequencer (pid $CANARY_LOCALNET_PID)" >&2 ;;
    scaffold)
      logos-scaffold localnet stop >&2 2>/dev/null || true ;;
  esac
}
