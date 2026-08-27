// Shared constants for the atomic-swaps offer channel, for whichever delivery
// fleet this process is pointed at.
//
// FLEET SELECTION — `SWAP_FLEET` (see FLEETS below):
//   unset (the default) or "logos.dev"  -> logos.dev,  cluster 3  [today's behaviour]
//   "logos.test"                        -> logos.test, cluster 2  [PR #125's fleet]
// Anything else throws at import time. There is deliberately NO silent
// fallback: every failure this area has produced — the Aug-7/8 cluster 2 -> 3
// re-genesis, an app on one fleet with makers on another — looks exactly the
// same from the outside, an empty offer board with no error, so a mistyped
// fleet must fail where it can still be seen.
//
// Why the knob exists: the app's fleet is compiled into the binary, so the
// app-side migration to logos.test (PR #125) can only land in a quiet release
// window. A maker, by contrast, is re-pointed by restarting one container —
// so a maker can be dual-homed onto both fleets first (run one publisher per
// fleet), which warms logos.test and lets #125 be proven end-to-end BEFORE it
// ships. Activating that is an operational step; this file only makes it
// possible. See deploy/docker-compose.multi.yml.
//
// Bootstrap resolution: neither fleet has an enrtree/DNS-discovery URL — the
// delivery module embeds a STATIC list of 6 multiaddrs per fleet and so do we.
// Both tables below are copied verbatim from the delivery_module v0.2.0 pin
// (logos-delivery f8b036594ea2a36b529e10b584b7d2851a3ac5c8,
// logos_delivery/waku/factory/networks_config.nim: `LogosDevConf.entryNodes`
// and `LogosTestConf.entryNodes`), peer-ids included.
//
// NOTE: both fleets expose raw TCP (port 30303) only. Node.js processes (the
// smoke test and the maker's offer-publisher sidecar) dial these directly via
// @libp2p/tcp. Browsers CANNOT dial raw TCP — the browser board needs a
// WSS-capable endpoint for the same fleet (see config.js / docs).

export const OFFERS_CONTENT_TOPIC = "/atomic-swaps/1/offers/json";

// RFQ (request-for-quote) request topic. Takers publish anonymous
// offer-requests here; makers filter-subscribe to it and respond
// (rate-limited) on OFFERS_CONTENT_TOPIC. See offer-publisher/rfq.mjs and
// swap-module/src/swap_delivery_adapter.cpp.
//
// Fleet-independent, like OFFERS_CONTENT_TOPIC: autosharding keys only on the
// content topic's application+version (both "atomic-swaps"/"1"), so every
// atomic-swaps topic lands on shard 7 of whichever cluster is selected below.
export const OFFER_REQUESTS_CONTENT_TOPIC = "/atomic-swaps/1/offer-requests/json";

/**
 * The two fleets a maker can publish on. Values are load-bearing in a way that
 * fails SILENTLY when wrong — a mismatched clusterId or peer-id produces an
 * empty board, not an error — so they are copied from the sources named in the
 * header rather than retyped.
 */
const FLEETS = {
  // logos.dev — where the live market runs today, and the fleet the shipped
  // app subscribes to.
  //
  // clusterId 3 is a deliberate DIVERGENCE from the delivery module's own
  // LogosDevConf (which still says clusterId: 2 at the v0.2.0 pin): the fleet
  // migrated cluster 2 -> cluster 3 during the Aug-7/8 2026 LEZ/delivery
  // upgrade and only serves cluster 3 now — cluster 2 subscribes are rejected
  // ("filter subscribe returned false" / "lightpush rejected by all peers").
  // Upstream caught up in delivery_module v0.2.1 (LogosDevConf.clusterId: 3);
  // our pin is v0.2.0, so we still force it here. Our content topics autoshard
  // to /waku/2/rs/3/7 under this config. This is exactly the hand-maintained
  // override that migrating to logos.test removes.
  "logos.dev": {
    networkConfig: { clusterId: 3, numShardsInCluster: 8 },
    tcpPeers: [
      "/dns4/delivery-01.do-ams3.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAmTUbnxLGT9JvV6mu9oPyDjqHK4Phs1VDJNUgESgNSkuby",
      "/dns4/delivery-02.do-ams3.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAmMK7PYygBtKUQ8EHp7EfaD3bCEsJrkFooK8RQ2PVpJprH",
      "/dns4/delivery-01.gc-us-central1-a.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm4S1JYkuzDKLKQvwgAhZKs9otxXqt8SCGtB4hoJP1S397",
      "/dns4/delivery-02.gc-us-central1-a.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm8Y9kgBNtjxvCnf1X6gnZJW5EGE4UwwCL3CCm55TwqBiH",
      "/dns4/delivery-01.ac-cn-hongkong-c.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm8YokiNun9BkeA1ZRmhLbtNUvcwRr64F69tYj9fkGyuEP",
      "/dns4/delivery-02.ac-cn-hongkong-c.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAkvwhGHKNry6LACrB8TmEFoCJKEX29XR5dDUzk3UT3UNSE",
    ],
  },

  // logos.test — the stability-guaranteed fleet upstream asks us to use
  // (logos-co/logos-delivery-module#84: "Please use logos.test instead…
  // logos.dev is subtle to change at any moment"). cluster 2 here is the
  // preset's OWN native cluster — no override, and it has not moved across
  // delivery_module 0.1.2 / 0.1.3 / 0.2.0 / 0.2.1. Our content topics
  // autoshard to /waku/2/rs/2/7 under this config. Parameters and peer-ids
  // are PR #125's, verified live on 2026-08-27 (fleet reachable, 6/6 dialed,
  // publish -> fleet -> filter round trip on the same shard).
  "logos.test": {
    networkConfig: { clusterId: 2, numShardsInCluster: 8 },
    tcpPeers: [
      "/dns4/node-01.do-ams3.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmQ9X2xDfPG3uL77V9piYDhjq14JhKCtcmNYsTMKNqrKCj",
      "/dns4/node-02.do-ams3.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmB8NYprrfQrgWVzsJtYWkfjsXbmJEGNMG6othXsQ53BwG",
      "/dns4/node-01.gc-us-central1-a.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmF8WtwGPmeGHgYAX2277jHgy5cW9F7zsB8EqUjBZQAZQ3",
      "/dns4/node-02.gc-us-central1-a.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmUuXhUW9bdJpzN1kfDziFiUZo4bszTk66cvr7uuyCHXR7",
      "/dns4/node-01.ac-cn-hongkong-c.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmL3oU95jh1BZHozn3uNhx8HEneirgr8M1jEAapzXGDqRF",
      "/dns4/node-02.ac-cn-hongkong-c.logos.test.status.im/tcp/30303/p2p/16Uiu2HAm28CoBZjpyxsanC8tQpbvZ7bZJnVYuB1EgFzb571qpWsV",
    ],
  },
};

/** Fleet used when SWAP_FLEET is unset — i.e. every maker running today. */
export const DEFAULT_FLEET = "logos.dev";

/** Every accepted SWAP_FLEET value, for error messages and docs. */
export const KNOWN_FLEETS = Object.keys(FLEETS);

/**
 * Resolve a SWAP_FLEET value to a fleet name.
 *
 * `undefined` (unset) means the default. ANY other value must name a known
 * fleet — including the empty string, which is a set-but-wrong variable, not
 * an unset one. Throws rather than falling back, on purpose: see the header.
 *
 * @param {string|undefined} raw process.env.SWAP_FLEET
 * @returns {string} a key of FLEETS
 */
export function resolveFleetName(raw) {
  if (raw === undefined) return DEFAULT_FLEET;
  const name = raw.trim();
  if (!Object.hasOwn(FLEETS, name)) {
    throw new Error(
      `SWAP_FLEET=${JSON.stringify(raw)} is not a known fleet. ` +
        `Known fleets: ${KNOWN_FLEETS.join(", ")}. ` +
        `Unset SWAP_FLEET to use the default (${DEFAULT_FLEET}).`
    );
  }
  return name;
}

/** The fleet this process publishes on, e.g. "logos.dev". */
export const FLEET_NAME = resolveFleetName(process.env.SWAP_FLEET);

export const NETWORK_CONFIG = FLEETS[FLEET_NAME].networkConfig;

/** Static bootstrap peers for FLEET_NAME (TCP — Node.js only). */
export const FLEET_TCP_PEERS = FLEETS[FLEET_NAME].tcpPeers;

/** Create a light node wired for the selected fleet over TCP (Node.js only). */
export async function createFleetNode() {
  const [{ createLightNode }, { tcp }] = await Promise.all([
    import("@waku/sdk"),
    import("@libp2p/tcp"),
  ]);
  return createLightNode({
    networkConfig: NETWORK_CONFIG,
    bootstrapPeers: FLEET_TCP_PEERS,
    libp2p: { transports: [tcp()] },
    // Node.js: no localStorage peer cache, no DNS discovery (fleet has no enrtree).
    discovery: { dns: false, peerExchange: true, peerCache: false },
  });
}

/** Dial every fleet peer; resolves with the number of successful dials. */
export async function dialFleet(node, log = () => {}) {
  const { multiaddr } = await import("@multiformats/multiaddr");
  let ok = 0;
  await Promise.all(
    FLEET_TCP_PEERS.map(async (addr) => {
      try {
        await node.dial(multiaddr(addr));
        ok += 1;
        log(`dialed ${addr.split("/")[2]}`);
      } catch (err) {
        log(`dial failed ${addr.split("/")[2]}: ${err.message}`);
      }
    })
  );
  return ok;
}
