# Installing the swap modules in Basecamp (community catalog)

The `swap` (backend) and `swap_ui` (UI) modules are published as `.lgx`
packages from **this repo's own GitHub releases**, indexed by this
repo's own catalog. You install them by adding the catalog URL to
Basecamp — no official-catalog listing required.

> Platforms: **darwin-arm64**, **linux-amd64**, **linux-arm64**. The
> release workflows build all three (issue #32 pinned the Linux circuit
> and rapidsnark hashes). Intel macOS is not supported — upstream ships
> no `macos-x86_64` circuits bundle.

> **Status: release imminent.** The LEZ v0.2.0 client repin has landed on
> `master`, so modules built from `master` talk to the public LEZ testnet
> listed below. No module release has been *dispatched* yet — until the
> first release workflow run is cut, the catalog URL resolves but offers
> no packages; this guide is a preview of the flow.
>
> The Linux legs have never been executed on a Linux runner, so treat
> linux-amd64 / linux-arm64 as unproven until the first release run is
> green. The matrix is `fail-fast: false`: if a Linux leg breaks, the
> release still publishes the variants that did build.

## Prerequisites

- **Logos Basecamp 0.2.1** — download from the
  [logos-basecamp releases page](https://github.com/logos-co/logos-basecamp/releases):
  - macOS Apple Silicon: the `aarch64.dmg`; open it and drag Basecamp to
    Applications.
  - Linux: the `x86_64.AppImage` or `aarch64.AppImage`; `chmod +x` it and
    run it.
- macOS on Apple Silicon (M1 or newer), or Linux on `x86_64` / `aarch64`.

## 1. Add the catalog

1. Open Basecamp → **Settings → Repositories**.
2. Paste this URL into the "Add repository" field and confirm:

   ```
   https://raw.githubusercontent.com/logos-co/eth-lez-atomic-swaps/master/logos-repo.json
   ```

3. The "ETH ↔ LEZ Atomic Swaps" repository appears and is merged with
   the built-in catalog.

## 2. Install swap_ui

1. Go to the package/module browser.
2. Install **swap_ui**. Basecamp resolves its declared dependencies —
   **swap** (from this same catalog) and **delivery_module** (from the
   official catalog) — automatically. If dependency resolution is not
   offered, install **swap** first, then **swap_ui**.
3. Restart Basecamp if the module doesn't appear immediately.

## 3. Configure the module

The swap module needs these endpoints/values:

| Setting | Value |
| --- | --- |
| LEZ sequencer RPC | `https://testnet.lez.logos.co` |
| LEZ swap program ID | `27720b5b0345135d8e684eb172c27f5fb237548cc891a3ec889d0ed340504070` (deployed on the public testnet 2026-07-21; matches the LEZ v0.2.0 client pin on `master`) |
| ETH HTLC contract (Sepolia) | `0x8636Fe66DFee166589a913140f14d5F57394834A` |
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
  v0.2.0 tag the module pins — see [`testnet.md`](https://github.com/logos-co/eth-lez-atomic-swaps/blob/master/docs/testnet.md) for
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
