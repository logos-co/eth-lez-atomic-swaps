#!/usr/bin/env bash
set -euo pipefail

# Build and exercise swap_ui as an installed module inside the real Basecamp
# bundle pinned by scaffold.toml. Basecamp's inspector build differs from the
# shipping bundle only by enabling logos-qt-mcp; package loading, ui-host,
# logos_host, and the application shell are the production code paths.

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
cd "$repo_root"

artifact_dir=${BASECAMP_UI_ARTIFACT_DIR:-"$repo_root/artifacts/basecamp-ui-smoke"}
mkdir -p "$artifact_dir"
artifact_dir=$(cd "$artifact_dir" && pwd -P)
exec > >(tee "$artifact_dir/harness.log") 2>&1

for command_name in nix node awk find; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "missing required command: $command_name" >&2
    exit 1
  }
done

toml_value() {
  local section=$1
  local key=$2
  awk -v wanted_section="[$section]" -v wanted_key="$key" '
    /^\[/ { in_section = ($0 == wanted_section) }
    in_section && $0 ~ "^[[:space:]]*" wanted_key "[[:space:]]*=" {
      value = $0
      sub(/^[^=]*=[[:space:]]*/, "", value)
      if (match(value, /^\"[^\"]*\"/)) {
        value = substr(value, RSTART + 1, RLENGTH - 2)
      } else {
        sub(/[[:space:]]*(#.*)?$/, "", value)
      }
      print value
      exit
    }
  ' scaffold.toml
}

basecamp_source=$(toml_value repos.basecamp source)
basecamp_pin=$(toml_value repos.basecamp pin)
lgpm_source=$(toml_value repos.lgpm source)
lgpm_pin=$(toml_value repos.lgpm pin)
delivery_flake=$(toml_value modules.delivery_module flake)

if [[ "$basecamp_source" != "https://github.com/logos-co/logos-basecamp.git" ]]; then
  echo "unsupported [repos.basecamp].source: $basecamp_source" >&2
  exit 1
fi
if [[ "$lgpm_source" != "github:logos-co/logos-package-manager" ]]; then
  echo "unsupported [repos.lgpm].source: $lgpm_source" >&2
  exit 1
fi
for pin_name in basecamp_pin lgpm_pin; do
  pin_value=${!pin_name}
  if [[ ! "$pin_value" =~ ^[0-9a-f]{40}$ ]]; then
    echo "$pin_name must be a full 40-character Git commit, got: $pin_value" >&2
    exit 1
  fi
done
if [[ -z "$delivery_flake" || "$delivery_flake" != *"#lgx" ]]; then
  echo "[modules.delivery_module].flake must end in #lgx, got: $delivery_flake" >&2
  exit 1
fi

basecamp_ref="github:logos-co/logos-basecamp/$basecamp_pin"
lgpm_ref="$lgpm_source/$lgpm_pin"
delivery_ref="${delivery_flake%#lgx}#lgx-portable"

build_one() {
  local label=$1
  local ref=$2
  shift 2
  local output
  echo "==> Building $label: $ref" >&2
  output=$(nix build "$ref" --no-link --print-out-paths "$@")
  if [[ $(printf '%s\n' "$output" | sed '/^$/d' | wc -l | tr -d ' ') != "1" ]]; then
    echo "$label produced an unexpected output list:" >&2
    printf '%s\n' "$output" >&2
    return 1
  fi
  printf '%s\n' "$output"
}

find_single_lgx() {
  local label=$1
  local output_dir=$2
  local matches=()
  while IFS= read -r candidate; do
    matches+=("$candidate")
  done < <(find "$output_dir" -maxdepth 1 -type f -name '*.lgx' -print)
  if [[ ${#matches[@]} -ne 1 ]]; then
    echo "$label output must contain exactly one .lgx, found ${#matches[@]} in $output_dir" >&2
    return 1
  fi
  printf '%s\n' "${matches[0]}"
}

# Build the same portable package variants that Basecamp consumes. The module
# flakes remain package-only; the host and test framework come from Basecamp.
# Keep the two scoped nixpkgs workarounds identical to the existing PR-time
# module-build job: the pinned builder and Delivery nixpkgs revisions predate
# crates.io's User-Agent fix. They change only the affected nested input, not
# this repo's flake outputs or host selection.
module_builder_nixpkgs='github:danisharora099/nixpkgs/eaec81d3b8a8d2339e25c718a1650b2c45adf726'
delivery_nixpkgs='github:danisharora099/nixpkgs/3134d7bb12629545b1f3e5b1d2faadbf861484fd'
delivery_overrides=(
  --override-input logos-delivery/nixpkgs "$delivery_nixpkgs"
)
swap_overrides=(
  --override-input logos-module-builder/nixpkgs "$module_builder_nixpkgs"
  --override-input delivery_module/logos-delivery/nixpkgs "$delivery_nixpkgs"
)
swap_ui_overrides=(
  --override-input logos-module-builder/nixpkgs "$module_builder_nixpkgs"
  --override-input swap/logos-module-builder/nixpkgs "$module_builder_nixpkgs"
  --override-input swap/delivery_module/logos-delivery/nixpkgs "$delivery_nixpkgs"
)
delivery_output=$(build_one delivery_module "$delivery_ref" "${delivery_overrides[@]}")
swap_output=$(build_one swap 'git+file:.?dir=swap-module#lgx-portable' "${swap_overrides[@]}")
swap_ui_output=$(build_one swap_ui 'git+file:.?dir=swap-ui#lgx-portable' "${swap_ui_overrides[@]}")
lgpm_output=$(build_one lgpm "$lgpm_ref#cli-portable")
basecamp_output=$(build_one 'Basecamp inspector bundle' "$basecamp_ref#bin-bundle-dir-inspector")
qt_mcp_output=$(build_one logos-qt-mcp "$basecamp_ref#logos-qt-mcp")

delivery_lgx=$(find_single_lgx delivery_module "$delivery_output")
swap_lgx=$(find_single_lgx swap "$swap_output")
swap_ui_lgx=$(find_single_lgx swap_ui "$swap_ui_output")
lgpm_bin="$lgpm_output/bin/lgpm"
basecamp_bin="$basecamp_output/bin/LogosBasecamp"

[[ -x "$lgpm_bin" ]] || { echo "lgpm binary missing: $lgpm_bin" >&2; exit 1; }
[[ -x "$basecamp_bin" ]] || { echo "Basecamp binary missing: $basecamp_bin" >&2; exit 1; }
[[ -f "$qt_mcp_output/test-framework/framework.mjs" ]] || {
  echo "logos-qt-mcp test framework missing from $qt_mcp_output" >&2
  exit 1
}

test_root=$(mktemp -d /tmp/atomic-swaps-basecamp-ui.XXXXXX)
user_dir="$test_root/user-dir"
runtime_dir="$test_root/runtime"
mkdir -p "$user_dir/modules" "$user_dir/plugins" "$runtime_dir"
chmod 700 "$runtime_dir"
[[ $(stat -c '%a' "$runtime_dir" 2>/dev/null || stat -f '%Lp' "$runtime_dir") == 700 ]] || {
  echo "XDG runtime directory is not owner-only: $runtime_dir" >&2
  exit 1
}

cleanup() {
  local rc=$?
  trap - EXIT
  if [[ -d "$user_dir/logs" ]]; then
    mkdir -p "$artifact_dir/basecamp-logs"
    cp -R "$user_dir/logs/." "$artifact_dir/basecamp-logs/" 2>/dev/null || true
  fi
  if [[ -d "$runtime_dir" ]]; then
    mkdir -p "$artifact_dir/runtime-logs"
    cp -R "$runtime_dir/." "$artifact_dir/runtime-logs/" 2>/dev/null || true
  fi
  if [[ ${BASECAMP_UI_KEEP_TMP:-0} == 1 ]]; then
    echo "Preserving test root: $test_root"
  elif [[ "$test_root" == /tmp/atomic-swaps-basecamp-ui.* && -d "$test_root" ]]; then
    rm -rf -- "$test_root"
  else
    echo "refusing to remove unexpected test root: $test_root" >&2
  fi
  exit "$rc"
}
trap cleanup EXIT

echo "==> Installing portable packages into $user_dir"
for lgx in "$delivery_lgx" "$swap_lgx" "$swap_ui_lgx"; do
  "$lgpm_bin" \
    --modules-dir "$user_dir/modules" \
    --ui-plugins-dir "$user_dir/plugins" \
    install --file "$lgx"
done

# Fail before launch if LGPM put an artifact in the wrong package class or
# stripped the dependency/view metadata Basecamp needs to discover it.
test -f "$user_dir/modules/delivery_module/manifest.json"
test -f "$user_dir/modules/swap/manifest.json"
test -f "$user_dir/plugins/swap_ui/manifest.json"
node - "$user_dir" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");
const root = process.argv[2];
const read = (kind, name) => JSON.parse(
  fs.readFileSync(path.join(root, kind, name, "manifest.json"), "utf8")
);
const delivery = read("modules", "delivery_module");
const swap = read("modules", "swap");
const ui = read("plugins", "swap_ui");
if (delivery.name !== "delivery_module" || delivery.type !== "core")
  throw new Error("delivery_module manifest is not an installed core module");
if (swap.name !== "swap" || swap.type !== "core" || !(swap.dependencies || []).includes("delivery_module"))
  throw new Error("swap manifest lost its delivery_module dependency");
if (ui.name !== "swap_ui" || ui.type !== "ui_qml" || !(ui.dependencies || []).includes("swap") || !ui.view)
  throw new Error("swap_ui manifest is not a Basecamp ui_qml plugin with its swap dependency and view");
NODE

echo "==> Launching the real pinned Basecamp bundle and running UI assertions"
BASECAMP_BIN="$basecamp_bin" \
BASECAMP_USER_DIR="$user_dir" \
BASECAMP_RUNTIME_DIR="$runtime_dir" \
BASECAMP_UI_ARTIFACT_DIR="$artifact_dir" \
LOGOS_QT_MCP="$qt_mcp_output" \
node tests/basecamp-ui-smoke.mjs
