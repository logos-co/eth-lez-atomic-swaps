# ETH ↔ LEZ Atomic Swaps

Trustlessly swap Sepolia ETH ↔ LEZ, peer-to-peer, inside Logos Basecamp — no custodian.

<!-- Screenshot goes here once docs/img/app.png is added:
![ETH ↔ LEZ Atomic Swaps](docs/img/app.png) -->

## Try it

1. In Basecamp, open **Settings → Repositories** and add this catalog:
   `https://logos.substratestudios.xyz/logos-repo.json`
2. Install **swap**, then **swap_ui** (current version 0.4.4). Keep the built-in
   catalog enabled — it provides the `delivery_module` dependency.
3. In the app, the **Setup** tab walks you through it: generate an Ethereum key,
   create a LEZ account, fund it, get test ETH — then open **Market** and accept
   an offer.

Full walkthrough:
[Swap ETH and LEZ tokens in Logos Basecamp](https://github.com/logos-co/logos-docs/blob/master/docs/basecamp/swap-eth-and-lez-tokens-in-logos-basecamp.md).
Install details and endpoints: [docs/community-install.md](docs/community-install.md).

## How it works

Each swap is a pair of hash time-locked contracts (HTLCs), one per chain. The
taker locks ETH on Sepolia; the maker verifies it and locks LEZ; the taker
claims the LEZ, revealing a secret; the maker uses that secret to claim the
ETH. Both sides complete, or the timelocks let everyone refund — nobody can
keep both assets.

```text
taker locks ETH ──▶ maker locks LEZ ──▶ taker claims LEZ (reveals secret)
                                    └─▶ maker claims ETH (using the secret)
```

## Develop

```bash
make basecamp-dev   # build the working tree and run it inside real Basecamp
make test           # full test suite (contracts + integration, via lgs)
make preflight      # fast local checks before you push — see docs/DEVELOPMENT.md
```

Where things live:

- `swap-module/` — the core swap backend (C++ module wrapping the Rust orchestrator)
- `swap-ui/` — the Basecamp UI
- `offer-publisher/` — the market-maker sidecar that keeps offers published
- `deploy/` — the maker fleet (liquidity-bot container)

Toolchain setup, the two-peer local flow, and build notes:
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Links

- [User walkthrough](https://github.com/logos-co/logos-docs/blob/master/docs/basecamp/swap-eth-and-lez-tokens-in-logos-basecamp.md) (journey doc)
- [Report feedback](https://github.com/logos-co/eth-lez-atomic-swaps/issues/new?template=trial-feedback.yml) · [issues](https://github.com/logos-co/eth-lez-atomic-swaps/issues)
- Running a market maker? Start at [deploy/README.md](deploy/README.md).

## Disclaimer

Testnet only (Sepolia + LEZ testnet), unaudited. This repository is part of an
experimental development environment and is not intended for production use —
see the [Logos Core repository](https://github.com/logos-co/logos-liblogos).
