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
node watch-offers.mjs                        # logos.dev — where the market is
SWAP_FLEET=logos.test node watch-offers.mjs  # the migration fleet
```

Expect one line per heartbeat (`OFFER_HEARTBEAT_SECS`, default 30s) showing
the maker's LEZ/ETH amounts and `age=0s` on arrival. The first log line names
the fleet being watched — check it before concluding a board is empty.

### Which fleet a maker publishes on (`SWAP_FLEET`)

Unset, every maker publishes on **logos.dev**, exactly as today. Setting
`SWAP_FLEET=logos.test` re-points one maker's publisher at the
stability-guaranteed fleet the app migrates to in PR #125 — see
"Dual-homing the market onto logos.test" below. An unrecognized value aborts
the publisher at startup instead of quietly picking a fleet.

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

## Running multiple makers (a real market, not one quote)

One maker publishes one offer. To make the board read like an actual market —
several concurrent offers with a visible spread of sizes AND rates — run
several maker instances. Each is a full copy of the same image with its OWN
LEZ account, ETH key, state volume, and offer terms; only the config differs.

`docker-compose.multi.yml` adds four more makers (`maker-2` … `maker-5`)
alongside the original. A suggested spread (the original `maker` sits at
10 LEZ ↔ 0.00001 ETH, 1:1M):

| instance  | container         | LEZ_AMOUNT | ETH_AMOUNT  | rate (LEZ per ETH) | flavour              |
| --------- | ----------------- | ---------- | ----------- | ------------------ | -------------------- |
| maker     | eth-lez-maker     | 10         | 0.00001     | 1,000,000 (1:1M)   | original / par       |
| maker-2   | eth-lez-maker-2   | 5          | 0.0000045   | 1,111,111          | starter, keen        |
| maker-3   | eth-lez-maker-3   | 25         | 0.000025    | 1,000,000 (1:1M)   | mid, par             |
| maker-4   | eth-lez-maker-4   | 50         | 0.000045    | 1,111,111          | larger, keen         |
| maker-5   | eth-lez-maker-5   | 100        | 0.00012     | 833,333            | whale, worse ask     |

`ETH_AMOUNT` is a decimal-ETH string (see `config::eth_to_wei`); the rate is
`LEZ_AMOUNT / ETH_AMOUNT`. A LOWER ETH per LEZ (a keener maker) is a HIGHER
"LEZ per ETH" number — that is the better deal for a taker.

### Why separate instances (not one process, many offers)

The maker binary runs exactly one offer per loop, and each offer must be
backed by a distinct escrow account: a distinct `ETH_RECIPIENT_ADDRESS` and
its own LEZ signing account holding that offer's inventory. So N offers = N
instances. They share only the immutable bits — the image, the fleet tables in
`offer-publisher/fleet.mjs` (which fleet each instance publishes on is
per-container, `SWAP_FLEET` — see step 4), and the public testnet endpoints.

> **HARD INVARIANT — distinct `ETH_RECIPIENT_ADDRESS` per maker, fleet-wide.**
> The maker matches an incoming ETH lock purely by `recipient ==
> ETH_RECIPIENT_ADDRESS` (`src/swap/maker.rs`). If two makers share a
> recipient, a *single* taker ETH lock matches BOTH: each locks its own LEZ
> bound to the taker, the taker claims every LEZ escrow with the one preimage,
> but only ONE maker can claim the single on-chain ETH escrow
> (first-writer-wins) — every other maker loses its LEZ for nothing. That is
> real fund loss the instant ≥2 makers collide, so each maker's freshly
> generated ETH key is BOTH its gas payer (`ETH_PRIVATE_KEY`) and its
> `ETH_RECIPIENT_ADDRESS`, and no two makers (including the original) may ever
> reuse one. Before starting a new maker, confirm its recipient is absent from
> every other running maker:
>
> ```bash
> grep -h '^ETH_RECIPIENT_ADDRESS=' maker.env maker-*.env | sort | uniq -d
> # ^ must print NOTHING (no duplicates). Same idea for LEZ_SIGNING_KEY.
> ```

No ports are published (every instance is outbound-only), so there is nothing
to port-shift between instances.

> **The offer-publisher source is bind-mounted, not baked.** The published maker
> image freezes `offer-publisher/` at build time, so when the fleet migrated
> cluster 2 → 3 the published image went stale — the live `eth-lez-maker` only
> survived because its in-container `fleet.mjs` was hand-patched. Both compose
> files therefore bind-mount the sidecar's modules — currently the repo's
> `../offer-publisher/{fleet.mjs,heartbeat.mjs,publish-offer.mjs,rfq.mjs}` —
> over the image copies (`node_modules` stays from the image), making the
> checked-out files the single source of truth: a fleet change (e.g. logos.test,
> PR #125), the RFQ on-demand responder (`publish-offer.mjs` + `rfq.mjs`,
> feat/rfq-on-demand-offers) **and** the heartbeat resolver (`heartbeat.mjs`)
> are picked up by a plain `restart`, no image rebuild or hand-patch. Mounting
> only `fleet.mjs` would run the image's older heartbeat-only publisher with no
> RFQ responder.
> **Make sure your checkout's `offer-publisher/` is current before starting** (an
> up-to-date `master` clone already is).

### 1. Provision each instance's accounts

Do this once per new maker, on a machine with CPU to spare — the pinata
proof-of-work is CPU-bound (difficulty 3), so the VPS, not a constrained
sandbox. All the tooling already exists:

```bash
# Fresh ETH key pair (address + private key) — one per maker, never reused:
cast wallet new

# Fresh LEZ account, initialized + pinata-funded to a target balance. The
# rust builder stage doubles as the provisioning tool (it builds the example;
# the shipped runtime image does not include it):
docker build --target rust-builder -t eth-lez-maker-builder ..
docker run --rm --network host eth-lez-maker-builder \
  cargo run --release --example onboard_maker_account -- \
  --sequencer-url https://testnet.lez.logos.co --target 150
```

`onboard_maker_account` prints the new LEZ account's base58 id and hex signing
key (the signing key is shown only once). Fund each account to at least its
`LEZ_AMOUNT` plus margin — one 150-LEZ pinata claim covers every instance
above except `maker-5` (100 LEZ, whose 3x low-water is 300), which wants two
claims (`--target 300`). The running loop then keeps each topped up
automatically (`FUND_LOW_WATER`, default 3x `LEZ_AMOUNT`).

The startup guard refuses to run a maker whose LEZ balance is below its
`LEZ_AMOUNT`, so LEZ funding is mandatory to go live. ETH is NOT checked at
startup — it is only spent on the profitable claim (~42k gas) when a swap
actually completes — so an instance publishes its offer fine with a thinly
funded (or even briefly empty) ETH key, but it needs enough ETH to claim
before a real taker completes a swap against it. Fund each ETH address thinly
(a handful of claims' worth is plenty on Sepolia).

### 2. Write each env file

Each instance reads a COMPLETE env file (compose loads one per service), so
copy the template and edit the four things that differ — the two keys, the
recipient address, and the offer terms:

```bash
for n in 2 3 4 5; do cp maker.env.example maker-$n.env; done
# then in each maker-$n.env set:
#   LEZ_SIGNING_KEY        = that instance's onboard signing key
#   ETH_PRIVATE_KEY        = that instance's `cast wallet new` private key
#   ETH_RECIPIENT_ADDRESS  = that instance's `cast wallet new` address (UNIQUE)
#   LEZ_AMOUNT / ETH_AMOUNT = the row from the table above
```

`maker-*.env` are gitignored (as is `maker.env`) — never commit them. Leave
`RESTRICT_COUNTERPARTY=false` and `LEZ_TAKER_ACCOUNT_ID` unset for public mode.

### 3. Start them (without touching the original)

```bash
docker compose -f docker-compose.multi.yml up -d
docker compose -f docker-compose.multi.yml logs -f
```

Use the `-f docker-compose.multi.yml` file ON ITS OWN — do NOT combine it with
`docker-compose.yml`. The multi file defines only `maker-2` … `maker-5` and
their volumes, so this brings the new makers up (and lets you `restart` / `down`
them) without ever recreating the live `eth-lez-maker`. Each logs
`offer published (N peer(s))` once the fleet accepts its heartbeat.

### 4. Dual-homing the market onto logos.test (optional, off by default)

The app's fleet is compiled into its binary, so the app can only move to
logos.test in a release; a maker moves with a container restart. Pointing one
maker at logos.test therefore puts a live publisher on the fleet **before** the
app switches, which is what lets the migration be proven end-to-end (its
runtime CI check asserts an offer arrives) instead of after it ships.

Per-container, durable across `up -d`:

```bash
# in deploy/ (.env is gitignored)
echo 'MAKER_5_FLEET=logos.test' >> .env
docker compose -f docker-compose.multi.yml up -d maker-5   # recreates ONLY maker-5
docker logs eth-lez-maker-5 | head -5                      # "connecting to logos.test fleet (cluster 2 -> /waku/2/rs/2/7 ...)"
```

Set `MAKER_<n>_FLEET` in `deploy/.env` or the shell — **not** in `maker-<n>.env`:
compose `environment` beats `env_file`, so a `SWAP_FLEET` line there is silently
ignored. Keep the clear majority of makers on logos.dev until the app migrates:
a taker on today's app only sees logos.dev offers.

### 5. Verify the market from outside

From your own machine (not the VPS), watch the fleet relay every maker's
offer — you should see all five distinct `maker_lez_account`s cycle through:

```bash
cd ../offer-publisher && npm ci && node watch-offers.mjs
```

If a maker was dual-homed above, it publishes on logos.test only, so it will
NOT appear here — watch it with
`SWAP_FLEET=logos.test node watch-offers.mjs` instead.
