#!/usr/bin/env bash
# Bridge for `lgs setup` against the LEZ v0.2.0 repo layout.
#
# logos-scaffold 0.1.1 hardcodes the wallet debug config path as
# `<lez repo>/wallet/configs/debug/wallet_config.json`, but the v0.2.0 repo
# moved the crate tree under a `lez/` subdirectory
# (`<lez repo>/lez/wallet/configs/debug/wallet_config.json`), so the wallet
# seeding step at the end of `logos-scaffold setup` fails with
# "missing wallet debug config in lez repo".
#
# Workaround: run setup once (clones the pinned repo, builds the toolchain),
# and if it fails, drop a `wallet -> lez/wallet` compatibility symlink into
# the pinned repo checkout and retry (idempotent — cached builds are reused).
# Remove this bridge when upstream scaffold understands the lez/ layout
# (tracked in docs/scaffold-upstream-tracker.md).
set -uo pipefail

cd "$(dirname "$0")/.."

LEZ_PIN=$(sed -n '/^\[repos\.lez\]/,/^\[/p' scaffold.toml | sed -n 's/^pin = "\([0-9a-f]*\)".*/\1/p' | head -1)
LEZ_REPO=".scaffold/lez-cache/repos/lez/${LEZ_PIN}"

link_wallet() {
  # logos-scaffold resolves wallet + sequencer paths from the repo root; the
  # v0.2.0 repo nests both under lez/.
  for d in wallet sequencer; do
    if [ -d "${LEZ_REPO}/lez/${d}" ] && [ ! -e "${LEZ_REPO}/${d}" ]; then
      ln -s "lez/${d}" "${LEZ_REPO}/${d}"
      echo "scaffold-setup: linked ${LEZ_REPO}/${d} -> lez/${d} (v0.2.0 layout bridge)"
    fi
  done
}

# The repo may already be cloned from a previous (failed) run.
link_wallet

if logos-scaffold setup; then
  exit 0
fi

echo "scaffold-setup: first setup attempt failed; applying v0.2.0 layout bridge and retrying" >&2
link_wallet
exec logos-scaffold setup
