// Shared constants for the atomic-swaps offer channel on the logos.dev fleet.
//
// Bootstrap resolution: the app's delivery module (preset "logos.dev",
// logos-delivery rev 509c8755) embeds a STATIC list of 6 fleet multiaddrs —
// there is no enrtree/DNS-discovery URL for this fleet. These are the same
// peers the shipped swap UI dials (verified against runtime logs).
//
// NOTE: the fleet exposes raw TCP (port 30303) only. Node.js processes (the
// smoke test and the maker's offer-publisher sidecar) dial these directly via
// @libp2p/tcp. Browsers CANNOT dial raw TCP — the browser board needs a
// WSS-capable endpoint for the same fleet (see config.js / docs).

export const OFFERS_CONTENT_TOPIC = "/atomic-swaps/1/offers/json";

// logos.dev preset: cluster 2, autosharding with 8 shards.
export const NETWORK_CONFIG = { clusterId: 2, numShardsInCluster: 8 };

// Static bootstrap peers baked into the logos.dev preset (TCP — Node.js only).
export const FLEET_TCP_PEERS = [
  "/dns4/delivery-01.do-ams3.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAmTUbnxLGT9JvV6mu9oPyDjqHK4Phs1VDJNUgESgNSkuby",
  "/dns4/delivery-02.do-ams3.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAmMK7PYygBtKUQ8EHp7EfaD3bCEsJrkFooK8RQ2PVpJprH",
  "/dns4/delivery-01.gc-us-central1-a.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm4S1JYkuzDKLKQvwgAhZKs9otxXqt8SCGtB4hoJP1S397",
  "/dns4/delivery-02.gc-us-central1-a.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm8Y9kgBNtjxvCnf1X6gnZJW5EGE4UwwCL3CCm55TwqBiH",
  "/dns4/delivery-01.ac-cn-hongkong-c.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm8YokiNun9BkeA1ZRmhLbtNUvcwRr64F69tYj9fkGyuEP",
  "/dns4/delivery-02.ac-cn-hongkong-c.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAkvwhGHKNry6LACrB8TmEFoCJKEX29XR5dDUzk3UT3UNSE",
];

/** Create a light node wired for the logos.dev fleet over TCP (Node.js only). */
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
