// Shared constants for the atomic-swaps offer channel on the logos.test fleet.
//
// Fleet choice: we target logos.test, the stability-guaranteed fleet, per
// upstream guidance (logos-co/logos-delivery-module#84: "Please use logos.test
// instead… logos.dev is subtle to change at any moment"). We previously rode
// logos.dev and had to force it onto Waku cluster 3 by hand after its Aug-7/8
// re-genesis migrated it off cluster 2 — that whole override era is gone:
// logos.test runs on its own native cluster 2 and is not moved out from under
// us. See PR "feat(fleet): migrate to logos.test".
//
// Bootstrap resolution: there is no enrtree/DNS-discovery URL for this fleet,
// so we embed a STATIC list of its 6 TCP multiaddrs. The peer-ids below were
// captured by dialing the fleet and reading each node's identify response
// (2026-08-11); node names are stable but confirm the peer-id if a dial starts
// failing.
//
// NOTE: the fleet exposes raw TCP (port 30303) only. Node.js processes (the
// smoke test and the maker's offer-publisher sidecar) dial these directly via
// @libp2p/tcp. Browsers CANNOT dial raw TCP — the browser board needs a
// WSS-capable endpoint for the same fleet (see config.js / docs).

export const OFFERS_CONTENT_TOPIC = "/atomic-swaps/1/offers/json";

// logos.test preset: cluster 2, autosharding with 8 shards. This is the
// preset's own native cluster — no override needed (unlike the logos.dev era).
// Our content topic autoshards to shard 7, i.e. pubsub topic /waku/2/rs/2/7.
export const NETWORK_CONFIG = { clusterId: 2, numShardsInCluster: 8 };

// Static bootstrap peers for the logos.test fleet (TCP — Node.js only).
export const FLEET_TCP_PEERS = [
  "/dns4/node-01.do-ams3.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmQ9X2xDfPG3uL77V9piYDhjq14JhKCtcmNYsTMKNqrKCj",
  "/dns4/node-02.do-ams3.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmB8NYprrfQrgWVzsJtYWkfjsXbmJEGNMG6othXsQ53BwG",
  "/dns4/node-01.gc-us-central1-a.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmF8WtwGPmeGHgYAX2277jHgy5cW9F7zsB8EqUjBZQAZQ3",
  "/dns4/node-02.gc-us-central1-a.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmUuXhUW9bdJpzN1kfDziFiUZo4bszTk66cvr7uuyCHXR7",
  "/dns4/node-01.ac-cn-hongkong-c.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmL3oU95jh1BZHozn3uNhx8HEneirgr8M1jEAapzXGDqRF",
  "/dns4/node-02.ac-cn-hongkong-c.logos.test.status.im/tcp/30303/p2p/16Uiu2HAm28CoBZjpyxsanC8tQpbvZ7bZJnVYuB1EgFzb571qpWsV",
];

/** Create a light node wired for the logos.test fleet over TCP (Node.js only). */
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
    // logos.test's cluster 2 is preset-native, so NETWORK_CONFIG carries it directly.
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
