#!/usr/bin/env bash
# Bridge for `lgs setup` against the LEZ v0.2.0 repo layout
# (upstream: https://github.com/logos-co/scaffold/issues/240).
#
# logos-scaffold hardcodes the wallet debug config path as
# `<lez repo>/wallet/configs/debug/wallet_config.json`, but the v0.2.0 repo
# moved the crate tree under a `lez/` subdirectory
# (`<lez repo>/lez/wallet/configs/debug/wallet_config.json`), so the wallet
# seeding step at the end of `logos-scaffold setup` fails with
# "missing wallet debug config in lez repo".
#
# Workaround: run setup once (clones the pinned repo, builds the toolchain),
# and if it fails, drop a `wallet -> lez/wallet` compatibility symlink into
# the pinned repo checkout and retry (idempotent — cached builds are reused).
#
# Second v0.2.0 gap, same upstream issue: the v0.2.0 debug wallet config no
# longer ships preconfigured `initial_accounts`, so scaffold's setup ends with
# "could not seed default wallet automatically" and never writes the
# `default_address=` line into .scaffold/state/wallet.state — which
# `lgs run`'s wallet-topup step requires. And the v0.2.0 wallet binary reads
# LEE_WALLET_HOME_DIR (scaffold still exports the older NSSA_WALLET_HOME_DIR),
# so without the explicit export below the wallet CLI would silently operate
# on ~/.lee/wallet instead of the project wallet. seed_default_wallet()
# replicates scaffold's seeding: initialize the project wallet storage (the
# wallet CLI creates a default Public/Private account pair on first use) and
# register the first public account as scaffold's default topup address.
#
# Remove this bridge when upstream scaffold understands the lez/ v0.2.0
# layout (scaffold#240; also tracked in docs/scaffold-upstream-tracker.md).
set -uo pipefail

cd "$(dirname "$0")/.."

# v0.2.0 wallet env name; scaffold's own subprocess env still uses NSSA_*.
export LEE_WALLET_HOME_DIR="$PWD/.scaffold/wallet"

seed_default_wallet() {
  local state=.scaffold/state/wallet.state
  if [ -f "$state" ] && grep -q '^default_address=' "$state"; then
    return 0
  fi
  # `wallet list` initializes the wallet storage on first use (creating a
  # default Public/Private account pair); scaffold pipes the wallet password
  # itself and LEE_WALLET_HOME_DIR (exported above) points it at the project
  # wallet.
  local addr
  addr=$(logos-scaffold wallet list 2>/dev/null \
    | grep -o 'Public/[1-9A-HJ-NP-Za-km-z]\{32,\}' | head -1)
  if [ -z "$addr" ]; then
    echo "scaffold-setup: could not seed a default wallet account (wallet list returned none)" >&2
    return 1
  fi
  # No `set -e` here, so an unchecked failure would be masked by the echo below
  # and leave wallet.state without a default_address — silently breaking
  # `lgs run`'s topup step later. Propagate it explicitly.
  if ! logos-scaffold wallet default set "$addr"; then
    echo "scaffold-setup: FAILED to set default wallet address ${addr} — 'lgs run' topup will have no default_address; inspect the wallet at ${LEE_WALLET_HOME_DIR}" >&2
    return 1
  fi
  echo "scaffold-setup: seeded default wallet address ${addr} (v0.2.0 seeding bridge)"
}

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
  seed_default_wallet
  exit $?
fi

echo "scaffold-setup: first setup attempt failed; applying v0.2.0 layout bridge and retrying" >&2
link_wallet
logos-scaffold setup || exit $?
seed_default_wallet
