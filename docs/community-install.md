# Installing the swap modules in Basecamp (community catalog)

The `swap` (backend) and `swap_ui` (UI) modules are published as `.lgx`
packages from **this repo's own GitHub releases**, indexed by this
repo's own catalog. You install them by adding the catalog URL to
Basecamp — no official-catalog listing required.

> Platform: **macOS Apple Silicon (darwin-arm64) only** for now. Linux
> builds are blocked on unpinned circuit hashes (issue #32).

## Prerequisites

- **Logos Basecamp 0.2.1** (macOS dmg) — download from the
  [logos-basecamp releases page](https://github.com/logos-co/logos-basecamp/releases),
  open the dmg and drag Basecamp to Applications.
- macOS on Apple Silicon (M1 or newer).

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
| LEZ swap program ID | **TODO** — will be filled in after the public-testnet program deploy completes |
| ETH HTLC contract (Sepolia) | `0x8636Fe66DFee166589a913140f14d5F57394834A` |
| ETH RPC (Sepolia, websocket) | `wss://ethereum-sepolia-rpc.publicnode.com` |

## 4. Funds

- **Sepolia ETH**: any Sepolia faucet works, e.g.
  [Google Cloud Web3 faucet](https://cloud.google.com/application/web3/faucet/ethereum/sepolia)
  or [sepoliafaucet.com](https://sepoliafaucet.com). You need a small
  amount of Sepolia ETH for HTLC funding + gas. Any standard ETH wallet
  key works; the module signs Sepolia transactions with the key you
  configure.
- **LEZ testnet funds**: use the public testnet faucet (pinata faucet
  for `testnet.lez.logos.co`) to fund your LEZ account.

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
