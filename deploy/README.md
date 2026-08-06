# Deploying the maker liquidity bot

Packages `swap-cli maker --loop` + its Node offer-publisher sidecar
(`offer-publisher/publish-offer.mjs`) as a single container. See the
`Dockerfile` at the repo root for the build; this directory has the runtime
pieces.

## Why a container

The build needs Rust 1.93 with network-fetching build scripts (LEZ v0.2.2
deps) **and** Node >=20 for `offer-publisher/`. A hermetic image is the only
thing reproducible on a bare box:

- **Nix** was rejected for this repo's CLI packaging — see issue #32
  (cargoHash churn on every dependency bump; the module packaging pipeline
  already pays this cost for the GUI, the CLI doesn't need to as well).
- **A bare systemd unit** was rejected because it leaves the Rust/Node
  toolchain as manual, drifting VPS state, and pins `WorkingDirectory` to a
  repo checkout (which is also why `.maker-state.json` must never live
  CWD-relative in this deployment — see below).

## Current mode: PUBLIC (serves any taker)

**This deployment runs in public mode: `RESTRICT_COUNTERPARTY=false` AND
`LEZ_TAKER_ACCOUNT_ID` unset (commented out) in `maker.env`.**

PR #64 (Sepolia `EthHTLC` ABI — `Locked` now carries `takerLezAccount`) and
PR #76 (engine binds each swap to that taker-supplied LEZ account) merged
into `master` on 2026-08-04, flipping `--restrict-counterparty` from a
mandatory flag to an opt-in allowlist — `RESTRICT_COUNTERPARTY` unset/false
is the correct, fully-supported **public** default (the loop learns each
taker's LEZ account from their own ETH lock; no static designated
counterparty needed). The matching Sepolia `EthHTLC` redeploy
(`0x351B0EA07739FA9F6769213927D7836a790A5FAF`, `INTERFACE_VERSION=2`) landed
in the same window — see `docs/testnet.md`.

### Gotcha: BOTH env vars gate restriction, not just `RESTRICT_COUNTERPARTY`

`--restrict-counterparty` (`RESTRICT_COUNTERPARTY`) only gates the STARTUP
validation (whether the flag/taker-id combination is internally consistent).
The runtime allowlist check is separate: `classify_candidate`'s
`designated_taker` is populated straight from `LEZ_TAKER_ACCOUNT_ID`
**whenever that env var is set at all** — it does NOT check
`RESTRICT_COUNTERPARTY`'s value. Concretely: if you flip a deployment from
restricted back to public by only changing `RESTRICT_COUNTERPARTY=true` to
`RESTRICT_COUNTERPARTY=false` but leave a stale `LEZ_TAKER_ACCOUNT_ID` in
`maker.env`, the maker silently keeps rejecting every taker except that one
account — it never becomes actually public. **Both** of the following are
required for public mode:

```bash
RESTRICT_COUNTERPARTY=false
#LEZ_TAKER_ACCOUNT_ID=...   # commented out / absent entirely, not just "false"
```

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
- LEZ HTLC program: `9eb88f51aae87a58fb74b8d2dc7327b39333585e63280e3f9cf8d86dac0ed702`

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

## Health & status (issue #93)

`docker compose ps` / `docker inspect --format='{{json .State.Health}}'
eth-lez-maker` report the container healthy based on
`swap-cli maker --status`, which reads a periodic JSON snapshot
(`MAKER_STATUS_FILE`, default `/app/state/maker-status.json`, rewritten every
~15s by the running loop) and asserts on it — it is deliberately NOT a bare
liveness proxy:

- unhealthy if the status file itself has gone stale (the writer/loop is
  wedged or dead),
- unhealthy if the last CONFIRMED offer publish (a lightpush send the fleet
  actually accepted, not just "the sidecar process exists") is older than 3x
  `OFFER_HEARTBEAT_SECS`,
- unhealthy if the offer-publisher sidecar isn't alive at all,
- unhealthy if the loop has exited (`stopped`/`cancelled`) while the
  container is still up.

Inspect it directly:

```bash
docker exec eth-lez-maker swap-cli maker --status
docker exec eth-lez-maker cat /app/state/maker-status.json
```

A permanently-red healthcheck is worse than none — it trains everyone to
ignore the signal — so if this ever starts failing, treat it as real signal,
not noise to relax away.

`failed` vs. `transient_errors`: the public testnet sequencer is known to
timeout intermittently (its reliability is not ours to fix). Every hot-path
balance read (the maker-loop's per-iteration check, the fund-topper, and the
startup inventory guard) is bounded-retried with backoff before it is
reported anywhere, so a sequencer blip that recovers within the retry budget
never shows up at all. If it doesn't recover, it increments
`transient_errors`, NOT `failed` — `failed` is reserved for genuine
swap-outcome failures (a refund, a claim error) so it stays a meaningful
signal even while the sequencer is flaky. If `transient_errors` is climbing
fast, that's the public sequencer having a bad day, not the bot.

## Operations

- Logs: `docker compose logs -f maker`
- Restart: `docker compose restart maker`
- Stop (graceful — SIGTERM lets swap-cli finish its current wait and reap
  the sidecar): `docker compose down`
- Top up LEZ inventory by hand: `swap-cli maker --fund-to <target>` run
  against the same account (see `src/lez/onboard.rs`). Not usually needed —
  the running loop already tops up automatically in the background whenever
  the LEZ balance drops below `FUND_LOW_WATER` (default 3x `LEZ_AMOUNT`),
  checked every `FUND_CHECK_SECS` (default 300s).
