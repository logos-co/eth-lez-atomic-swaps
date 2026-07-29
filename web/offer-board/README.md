# LEZ / ETH Offer Board

Static single-page board rendering live atomic-swap offers from the Waku
offers channel (`/atomic-swaps/1/offers/json`) on the **logos.dev** delivery
fleet (cluster 2, autosharding, 8 shards). Read-only browser light node:
filter-subscribe + render; no backend, no keys.

This directory also contains two Node.js tools that share the same fleet
constants (`fleet.mjs`):

| File | Purpose |
|---|---|
| `smoke.mjs` | Connectivity smoke test: dial the fleet, filter-subscribe, lightpush a canary offer, assert it comes back. `npm run smoke` |
| `publish-offer.mjs` | The maker bot's heartbeat sidecar — spawned by `swap-cli maker --loop`, republishes the offer every `OFFER_HEARTBEAT_SECS` (fleet runs `store=false`, so only live messages are visible). |
| `app.js` / `index.html` / `config.js` | The browser board itself. `npm run build` bundles into `dist/`. |

## Bootstrap resolution

The app's delivery module uses the `logos.dev` preset, which embeds a
**static list of 6 fleet multiaddrs** (no enrtree / DNS discovery):

```
/dns4/delivery-0{1,2}.do-ams3.logos.dev.status.im/tcp/30303/p2p/...
/dns4/delivery-0{1,2}.gc-us-central1-a.logos.dev.status.im/tcp/30303/p2p/...
/dns4/delivery-0{1,2}.ac-cn-hongkong-c.logos.dev.status.im/tcp/30303/p2p/...
```

(Extracted from the compiled `liblogosdelivery.dylib` of the pinned
`logos-delivery-module` v0.1.1 dependency and confirmed against runtime dial
logs. Full list in `fleet.mjs`.)

## BLOCKED: browser connectivity needs a WSS listener

The fleet peers listen on **raw TCP only** (`/tcp/30303`; ports 443/8000/8443
probed closed). Browsers cannot dial raw TCP — a browser Waku light node
requires a secure-websocket (`/wss`) transport.

- **Node.js connectivity is verified working** (`npm run smoke` performs a
  full publish→fleet→subscribe round-trip using `@libp2p/tcp`).
- **Browser connectivity is blocked on infra.** Ask for the fleet operators:
  *"Please expose a WSS listener (e.g. `/tcp/443/wss` behind TLS) on the
  cluster-2 logos.dev delivery nodes, or provide any WSS-capable cluster-2
  peer serving filter + lightpush, and share its multiaddr."*
- Once a WSS multiaddr exists, add it to `bootstrapPeers` in the deployed
  `config.js` — no rebuild needed. Until then the board loads and shows a
  "blocked" status banner with this explanation.

## Develop / deploy

```sh
npm install
npm run build          # bundles app.js -> dist/ (esbuild), copies index.html + config.js
npx serve dist         # local preview
npm run smoke          # Node-side fleet connectivity test
```

Deployed to GitHub Pages by `.github/workflows/deploy-board.yml` on pushes to
`master` touching `web/offer-board/**`.
