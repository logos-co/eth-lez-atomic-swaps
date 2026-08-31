# Offer-publisher sidecar

A **headless Node.js daemon** — not a website — spawned and supervised by the
liquidity bot (`swap-cli maker --loop`). It connects once to a delivery fleet
(over raw TCP via `@libp2p/tcp`) and **republishes the maker's offer** every
`OFFER_HEARTBEAT_SECS` seconds (default 30) with fresh absolute timelocks.
That is the single heartbeat knob end to end: `swap-cli maker` resolves it and
injects the resolved value here. `FALLBACK_HEARTBEAT_SECS` is a **deprecated**
alias that still works, warns at startup, and only applies when
`OFFER_HEARTBEAT_SECS` is unset — `heartbeat.mjs` owns that precedence and
`heartbeat.test.mjs` pins it.

## Which fleet: `SWAP_FLEET`

| `SWAP_FLEET` | fleet | cluster | pubsub topic |
| --- | --- | --- | --- |
| unset (default) or `logos.dev` | logos.dev | 3 | `/waku/2/rs/3/7` |
| `logos.test` | logos.test | 2 | `/waku/2/rs/2/7` |

Every script here honours it (`publish-offer.mjs`, `watch-offers.mjs`,
`smoke.mjs`, `smoke-rfq.mjs`) and prints the fleet it chose on startup. Any
other value — including an empty one — **aborts at import**; there is no
fallback, because a maker on the wrong fleet publishes into the void while
logging success. `fleet.test.mjs` pins both tables and that failure.

The live market and the shipped app are on **logos.dev**; `logos.test` exists
so a maker can be pointed at the migration fleet ahead of the app-side switch
(PR #125), which is what makes that migration provable end-to-end. One process
serves one fleet — dual-homing means running one publisher per fleet. Wiring
for the container case: `deploy/docker-compose.multi.yml`.

## Why a separate Node process (and not pure Rust)

The fleet runs `store=false`, so late-joining subscribers only ever see *live*
messages — the offer has to be re-broadcast on an interval or it vanishes. The
Rust `swap-cli` has **no delivery/Waku client linked in** (it coordinates every
swap purely on-chain), so there is currently no in-process way to lightpush the
offer. This sidecar is the delivery path. A pure-Rust republish (linking the
`logos-delivery` client into the CLI) is the intended follow-up that would let
us delete this process; until then Node is a hard runtime dependency of the
`--loop` heartbeat only. The heartbeat is best-effort: if this process is
absent or failing, swaps still complete (they coordinate on-chain) — only the
offer advertisement stops.

> The **browser offer board** (the viewer UI) previously lived alongside this
> script under `web/offer-board/`. It has moved into the `swap_ui` Basecamp app
> (home screen) and is no longer part of this repo.

## Setup

```sh
cd offer-publisher
npm install
```

Requires Node.js >= 22. (Previously documented as >= 20, but the current
`@waku/sdk`/libp2p dependency chain uses `Promise.withResolvers`
(`@libp2p/peer-store` -> `mortice` -> `it-queue`), which Node 20.x does not
provide — confirmed live: `node:20-bookworm-slim` throws
`TypeError: Promise.withResolvers is not a function` at runtime. Found while
containerizing the maker bot, see `Dockerfile`.)

## Usage

The bot spawns `node publish-offer.mjs` automatically; you rarely run it by
hand. It reads its offer parameters from environment variables set by
`swap-cli` (see the header of `publish-offer.mjs`). Override the path the bot
uses with `--publisher-script` / `OFFER_PUBLISHER_SCRIPT`.

Connectivity smoke test (publish -> fleet -> subscribe round-trip):

```sh
npm run smoke                        # logos.dev (default)
SWAP_FLEET=logos.test npm run smoke  # the migration fleet
```

## Files

- `publish-offer.mjs` — the heartbeat sidecar (spawned by `swap-cli maker --loop`).
- `fleet.mjs` — the `SWAP_FLEET` fleet table (logos.dev / logos.test) + TCP node/dial helpers (Node.js only).
- `fleet.test.mjs` — contract tests for that table and the selector (`node fleet.test.mjs`, no network).
- `smoke.mjs` — standalone fleet-connectivity check (`npm run smoke`).
