# Installing the swap modules in Basecamp (community catalog)

The `swap` (backend) and `swap_ui` (UI) modules are published as `.lgx`
packages from **this repo's own GitHub releases**, indexed by this
repo's own catalog. You install them by adding the catalog URL to
Basecamp — no official-catalog listing required.

> Platforms: **darwin-arm64**, **linux-amd64**, **linux-arm64**. The
> release workflows build all three (issue #32 pinned the Linux circuit
> and rapidsnark hashes). Intel macOS is not supported — upstream ships
> no `macos-x86_64` circuits bundle.

> **Current release (as of 2026-08-27): `swap v0.4.5` / `swap_ui v0.4.5`**,
> both published 2026-08-27. Both sidecars report
> `builtVariants: [darwin-arm64, linux-amd64, linux-arm64]` and
> `missingVariants: []` — all three platforms are built and published, and
> `.github/workflows/build-modules.yml` compiles both modules on
> `ubuntu-latest` and `ubuntu-24.04-arm` (in addition to macOS) on every
> pull request and every push to `master`, so the Linux legs are exercised
> continuously, not just at release time.

## Prerequisites

- **Logos Basecamp 0.2.3** (0.2.2 also works) — download from the
  [logos-basecamp releases page](https://github.com/logos-co/logos-basecamp/releases):
  - macOS Apple Silicon: the `aarch64.dmg`; open it and drag Basecamp to
    Applications.
  - Linux: the `x86_64.AppImage` or `aarch64.AppImage`; `chmod +x` it and
    run it.
- macOS on Apple Silicon (M1 or newer), or Linux on `x86_64` / `aarch64`.

> **Why 0.2.3, and why 0.2.2 is fine.** 0.2.3 is what `scaffold.toml` pins
> (`aa237766baf61404e12da86b7303cb41065464c9`, the upstream `0.2.3` tag) and
> what the `basecamp-ui-runtime` CI job builds, installs these modules into,
> and drives the UI of on every pull request — so it's the one to reach for.
> 0.2.2 locks `logos-package`, `logos-package-manager`,
> `logos-capability-module` and `logos-view-module-runtime` at exactly the
> same revisions, so the packaging, hash-validation and module-loading paths
> an install goes through are unchanged there. Nothing here covers 0.2.1 or
> earlier — don't assume they work.
>
> That CI proof runs on **linux-amd64** and installs **locally built** `.lgx`
> packages. macOS, and the catalog-download path in step 1, are exercised by
> hand, not by automation.

## 1. Add the catalog

1. Open Basecamp → **Settings → Repositories**.
2. Paste this URL into the "Add repository" field and confirm:

   ```
   https://raw.githubusercontent.com/logos-co/eth-lez-atomic-swaps/master/logos-repo.json
   ```

   This is the canonical catalog URL and the one to use — it's what
   `canary/leg-catalog.sh` verifies on every nightly run, and it doesn't
   depend on any one person's domain.

   A broader personal mirror also exists at
   `https://logos.substratestudios.xyz/logos-repo.json`. It carries the
   same `swap` / `swap_ui` releases alongside other unrelated apps, but
   it is not the supported path and no nightly check covers it. Reach
   for it only if you specifically want one of those other apps.

3. The "ETH ↔ LEZ Atomic Swaps" repository appears and is merged with
   the built-in catalog. **Keep the built-in/default repository
   enabled** — `swap`'s `delivery_module` dependency resolves from the
   official Logos catalog, not from this one, so disabling it breaks
   installation.

## 2. Install swap (core) before swap_ui

**Install order matters: `swap` (the core module) before `swap_ui` (the
UI module).** A UI module installed first will not load.

1. Go to the package/module browser.
2. Install **swap** first — it is the module that declares
   **delivery_module** (from the official catalog) as a dependency. Then
   install **swap_ui**, whose only declared dependency is `swap`. Basecamp
   may resolve both automatically, but do not rely on that if it installs
   `swap_ui` before `swap` is present.
3. Restart Basecamp if a module doesn't appear immediately.

## 3. Configure the module

The swap module needs these endpoints/values:

| Setting | Value |
| --- | --- |
| LEZ sequencer RPC | `https://testnet.lez.logos.co` |
| LEZ swap program ID | `9eb88f51aae87a58fb74b8d2dc7327b39333585e63280e3f9cf8d86dac0ed702` (deployed on the public testnet 2026-07-21; matches the LEZ v0.2.2 client pin on `master`) |
| ETH HTLC contract (Sepolia) | `0x351B0EA07739FA9F6769213927D7836a790A5FAF` (INTERFACE_VERSION 2) |
| ETH RPC (Sepolia, websocket) | `wss://ethereum-sepolia-rpc.publicnode.com` |

The ETH RPC **must** be a WebSocket (`wss://`) endpoint — plain
`https://` RPCs fail to connect.

Beyond the endpoints, the Config tab needs your own identities before
`validateConfig` passes:

- **ETH private key** — hex key of a Sepolia account you control (make a
  throwaway: any wallet, or `cast wallet new` if you have Foundry). It
  signs your HTLC lock/claim transactions.
- **ETH recipient address** — where the counterparty's ETH should land
  (usually the same account's address).
- **LEZ signing key** — a raw LEZ account key. Easiest path: create an
  account with the LEZ `wallet` CLI pointed at the public testnet
  (`wallet config set sequencer_addr https://testnet.lez.logos.co`, then
  create + init an account) and paste its signing key. Alternatively
  point the module at a wallet home dir + account ID.
- **Taker account ID** (taker role) — the LEZ account that receives the
  maker's LEZ.

Both LEZ accounts must be **initialized and funded on-chain** before a
swap (see Funds below). See [`testnet.md`](https://github.com/logos-co/eth-lez-atomic-swaps/blob/master/docs/testnet.md) for the exact
wallet-CLI commands.

## 4. Funds

- **Sepolia ETH**: any Sepolia faucet works, e.g.
  [Google Cloud Web3 faucet](https://cloud.google.com/application/web3/faucet/ethereum/sepolia)
  or [sepoliafaucet.com](https://sepoliafaucet.com). You need a small
  amount of Sepolia ETH for HTLC funding + gas. Any standard ETH wallet
  key works; the module signs Sepolia transactions with the key you
  configure.
- **LEZ testnet funds**: the public testnet's pinata faucet credits
  150 LEZ per claim, permissionlessly:

  ```
  wallet pinata claim --to <your-account-id>
  ```

  Repeat until funded (a maker locking 1000 LEZ needs ≥7 claims). The
  `wallet` CLI comes from the `logos-execution-zone` repo at the same
  v0.2.2 tag the module pins — see [`testnet.md`](https://github.com/logos-co/eth-lez-atomic-swaps/blob/master/docs/testnet.md) for
  building it and for account create/init commands.

## How this catalog works (for maintainers)

- `.github/workflows/release-swap.yml` / `release-swap-ui.yml`
  (manual dispatch) build `swap-module/` / `swap-ui/` via the shared
  [logos-modules-release-action](https://github.com/logos-co/logos-modules-release-action)
  pipeline and publish `swap-v<version>` / `swap_ui-v<version>` releases
  carrying the `.lgx`. Version source of truth: each module dir's
  `metadata.json`.
- `.github/workflows/rebuild-index.yml` regenerates `index.json` from
  all `.lgx` release assets and uploads it to the rolling
  [`index` release](https://github.com/logos-co/eth-lez-atomic-swaps/releases/tag/index).
- `/logos-repo.json` (repo root) is the stable entry point users paste
  into Basecamp; its `indexUrl` points at the rolling index asset.
