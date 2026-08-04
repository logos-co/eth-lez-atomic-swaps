# Deploying the maker liquidity bot

Packages `swap-cli maker --loop` + its Node offer-publisher sidecar
(`offer-publisher/publish-offer.mjs`) as a single container. See the
`Dockerfile` at the repo root for the build; this directory has the runtime
pieces.

## Why a container

The build needs Rust 1.93 with network-fetching build scripts (LEZ v0.2.0
deps) **and** Node >=20 for `offer-publisher/`. A hermetic image is the only
thing reproducible on a bare box:

- **Nix** was rejected for this repo's CLI packaging — see issue #32
  (cargoHash churn on every dependency bump; the module packaging pipeline
  already pays this cost for the GUI, the CLI doesn't need to as well).
- **A bare systemd unit** was rejected because it leaves the Rust/Node
  toolchain as manual, drifting VPS state, and pins `WorkingDirectory` to a
  repo checkout (which is also why `.maker-state.json` must never live
  CWD-relative in this deployment — see below).

## Current mode: RESTRICTED counterparty

**This deployment runs in restricted (single-taker) mode by operator
choice, not because the code requires it.**

Historical note, since this changed mid-flight: PR #64 (Sepolia `EthHTLC`
ABI — `Locked` now carries `takerLezAccount`) and PR #76 (engine binds each
swap to that taker-supplied LEZ account) merged into `master` on 2026-08-04,
*while this container was being built*, flipping `--restrict-counterparty`
from a mandatory flag to an opt-in allowlist — `RESTRICT_COUNTERPARTY`
unset/false is now the correct, fully-supported **public** default (the
loop learns each taker's LEZ account from their own ETH lock; no static
designated counterparty needed). The matching Sepolia `EthHTLC` redeploy
(`0x351B0EA07739FA9F6769213927D7836a790A5FAF`, `INTERFACE_VERSION=2`) landed
in the same window — see `docs/testnet.md`.

This deployment nonetheless keeps `RESTRICT_COUNTERPARTY=true` for now,
serving only the designated taker in `LEZ_TAKER_ACCOUNT_ID`, to keep the
first live rollout conservative while the new public-taker path gets more
runway. Flipping to public mode is a single env change — set
`RESTRICT_COUNTERPARTY=false` (or drop the var) in `maker.env` and restart
the container — no image rebuild, no Dockerfile change. The image itself
never bakes `--restrict-counterparty` in as a fixed CLI flag for exactly
this reason (see the `Dockerfile` CMD comment).

**If you rebuild this image from an older commit** (pre-2026-08-04, before
PR #64/#76), `--restrict-counterparty` is still mandatory there and
`ETH_HTLC_ADDRESS` must point at the old, now-superseded
`0x8636Fe66DFee166589a913140f14d5F57394834A` instead — the two ABIs are not
interchangeable.

## Hard rule: one `ETH_RECIPIENT_ADDRESS` per maker

Every maker instance/deployment needs a **distinct**
`ETH_RECIPIENT_ADDRESS`. If two makers share one address, both match the
same on-chain `Locked` event and race to lock the LEZ escrow; N-1 of them
lose that race and are left holding a journal entry for an escrow they
cannot refund ("only maker can refund" — the LEZ HTLC gates refund on the
account that locked it). `reconcile` retains that entry as a permanently
quarantined, unusable hashlock forever (see `src/lez/onboard.rs` /
`src/cli/bot.rs` quarantine handling). This was learned the expensive way —
do not reuse an `ETH_RECIPIENT_ADDRESS` across deployments.

## One-time setup

### 1. Generate a fresh Sepolia key for the bot

Never reuse a personal/dev key. Generate one on the VPS (or anywhere) and
fund it with ~0.05 ETH — its only gas cost is the profitable ETH claim
(~55k gas):

```bash
cast wallet new
```

Put the private key in `deploy/maker.env` as `ETH_PRIVATE_KEY` and the
address as `ETH_RECIPIENT_ADDRESS`.

### 2. Create + initialize + fund a LEZ account

Uses the native onboarding path from `src/lez/onboard.rs`
(`Signer::generate`, `ensure_initialized`, `claim_to_target`) — no scaffold,
no `wallet` binary. Run this via the same `swap-cli` binary the image ships
(e.g. `docker run --rm <image> swap-cli account create ...` — see `swap-cli
--help` for the exact subcommand, or drive it through `lez-mcp`).

Pinata (faucet) proof-of-work is CPU-bound at difficulty 3 — do this on the
VPS (4 cores), not a constrained sandbox: 150 LEZ per claim, so reaching a
useful balance takes a few claims.

Put the resulting signing key in `deploy/maker.env` as `LEZ_SIGNING_KEY`.

### 3. Configure `maker.env`

```bash
cp deploy/maker.env.example deploy/maker.env
# fill in ETH_PRIVATE_KEY, ETH_RECIPIENT_ADDRESS, LEZ_SIGNING_KEY,
# LEZ_TAKER_ACCOUNT_ID
```

`deploy/maker.env` is gitignored — never commit it.

Public testnet endpoints (already defaulted in `maker.env.example`):

- LEZ sequencer: `https://testnet.lez.logos.co`
- ETH RPC: `wss://ethereum-sepolia-rpc.publicnode.com`
- ETH HTLC (v2, current): `0x351B0EA07739FA9F6769213927D7836a790A5FAF`
- LEZ HTLC program: `27720b5b0345135d8e684eb172c27f5fb237548cc891a3ec889d0ed340504070`

## Build + run

Build on the host (no registry push required for a first deploy):

```bash
cd deploy
docker compose build
docker compose up -d
docker compose logs -f maker
```

`docker-compose.yml` declares both `image:` and `build:` for the same
service, so this works unchanged later too: once
`.github/workflows/release-maker-image.yml` has published a tag to GHCR,
`docker compose pull && docker compose up -d` picks up the registry image
instead — no file edits needed either way.

State (the crash-recovery journal `.maker-state.json` and any status file)
lives on the named `maker-state` volume, mounted at `/app/state` — never a
bind mount into a repo checkout, so it survives image upgrades and
container recreation.

## Verifying offers actually reach the fleet

The fleet runs `store=false` — an offer it never carries is worthless,
nothing is retained. `offer-publisher/watch-offers.mjs` subscribes to
`/atomic-swaps/1/offers/json` independently of the maker and prints each
offer with its age. Run it from **your own machine, not the VPS** (an
outside vantage point is the actual proof the fleet is relaying, not just
that the maker process is alive):

```bash
cd offer-publisher
npm ci
node watch-offers.mjs
```

Expect one line per heartbeat (`OFFER_HEARTBEAT_SECS`, default 45s) showing
the maker's LEZ/ETH amounts and `age=0s` on arrival.

## Operations

- Logs: `docker compose logs -f maker`
- Restart: `docker compose restart maker`
- Stop (graceful — SIGTERM lets swap-cli finish its current wait and reap
  the sidecar): `docker compose down`
- Top up LEZ inventory without restarting: `swap-cli maker --fund-to
  <target>` run against the same account (see `src/lez/onboard.rs`).
