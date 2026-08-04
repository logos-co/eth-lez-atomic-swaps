# Offer-publisher sidecar

A **headless Node.js daemon** — not a website — spawned and supervised by the
liquidity bot (`swap-cli maker --loop`). It connects once to the logos.dev
delivery fleet (cluster 2, over raw TCP via `@libp2p/tcp`) and **republishes the
maker's offer** every `OFFER_HEARTBEAT_SECS` seconds with fresh absolute
timelocks.

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
npm run smoke
```

## Files

- `publish-offer.mjs` — the heartbeat sidecar (spawned by `swap-cli maker --loop`).
- `fleet.mjs` — logos.dev fleet constants + TCP node/dial helpers (Node.js only).
- `smoke.mjs` — standalone fleet-connectivity check (`npm run smoke`).
