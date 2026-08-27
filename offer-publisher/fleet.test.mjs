// Contract tests for the SWAP_FLEET fleet selector (no Waku node, no network).
// Run: node --test offer-publisher/fleet.test.mjs
//
// What this guards, and why it is worth guarding: a wrong fleet parameter is
// INVISIBLE. Publish on the wrong cluster, or dial the wrong peers, and every
// log line still says "connected"/"published" — the only symptom is an offer
// board that stays empty for everyone. So the expected values below are
// written out in full rather than derived from the module under test:
//
//   * the logos.dev table is master's pre-SWAP_FLEET fleet.mjs, verbatim, so
//     any drift in TODAY'S live behaviour fails here rather than on the board;
//   * the logos.test table is PR #125's fleet.mjs, verbatim.
//
// Both were independently confirmed byte-identical to delivery_module v0.2.0's
// presets (logos-delivery f8b036594ea2a36b529e10b584b7d2851a3ac5c8,
// networks_config.nim: LogosDevConf/LogosTestConf entryNodes) — except
// logos.dev's clusterId, which we deliberately force to 3 (see fleet.mjs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

const MODULE_URL = new URL("./fleet.mjs", import.meta.url).href;

// ESM caches by URL, and fleet.mjs reads SWAP_FLEET once at evaluation time —
// so each case needs its own URL to force a fresh evaluation.
let evaluations = 0;
async function importWithFleet(value) {
  if (value === undefined) delete process.env.SWAP_FLEET;
  else process.env.SWAP_FLEET = value;
  try {
    return await import(`${MODULE_URL}?case=${(evaluations += 1)}`);
  } finally {
    delete process.env.SWAP_FLEET;
  }
}

// Autosharding, as the delivery module computes it (sharding.nim:20-30):
// shard = big-endian uint64 of sha256(application + version)[24..31] mod N.
// Both content topics are /atomic-swaps/1/... => application "atomic-swaps",
// version "1" => shard 7 on an 8-shard cluster.
function autoshard(application, version, numShardsInCluster) {
  const digest = createHash("sha256").update(application + version).digest();
  return Number(digest.readBigUInt64BE(24) % BigInt(numShardsInCluster));
}
const pubsubTopic = (cfg) =>
  `/waku/2/rs/${cfg.clusterId}/${autoshard("atomic-swaps", "1", cfg.numShardsInCluster)}`;

const LOGOS_DEV = {
  networkConfig: { clusterId: 3, numShardsInCluster: 8 },
  pubsubTopic: "/waku/2/rs/3/7",
  tcpPeers: [
    "/dns4/delivery-01.do-ams3.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAmTUbnxLGT9JvV6mu9oPyDjqHK4Phs1VDJNUgESgNSkuby",
    "/dns4/delivery-02.do-ams3.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAmMK7PYygBtKUQ8EHp7EfaD3bCEsJrkFooK8RQ2PVpJprH",
    "/dns4/delivery-01.gc-us-central1-a.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm4S1JYkuzDKLKQvwgAhZKs9otxXqt8SCGtB4hoJP1S397",
    "/dns4/delivery-02.gc-us-central1-a.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm8Y9kgBNtjxvCnf1X6gnZJW5EGE4UwwCL3CCm55TwqBiH",
    "/dns4/delivery-01.ac-cn-hongkong-c.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAm8YokiNun9BkeA1ZRmhLbtNUvcwRr64F69tYj9fkGyuEP",
    "/dns4/delivery-02.ac-cn-hongkong-c.logos.dev.status.im/tcp/30303/p2p/16Uiu2HAkvwhGHKNry6LACrB8TmEFoCJKEX29XR5dDUzk3UT3UNSE",
  ],
};

const LOGOS_TEST = {
  networkConfig: { clusterId: 2, numShardsInCluster: 8 },
  pubsubTopic: "/waku/2/rs/2/7",
  tcpPeers: [
    "/dns4/node-01.do-ams3.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmQ9X2xDfPG3uL77V9piYDhjq14JhKCtcmNYsTMKNqrKCj",
    "/dns4/node-02.do-ams3.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmB8NYprrfQrgWVzsJtYWkfjsXbmJEGNMG6othXsQ53BwG",
    "/dns4/node-01.gc-us-central1-a.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmF8WtwGPmeGHgYAX2277jHgy5cW9F7zsB8EqUjBZQAZQ3",
    "/dns4/node-02.gc-us-central1-a.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmUuXhUW9bdJpzN1kfDziFiUZo4bszTk66cvr7uuyCHXR7",
    "/dns4/node-01.ac-cn-hongkong-c.logos.test.status.im/tcp/30303/p2p/16Uiu2HAmL3oU95jh1BZHozn3uNhx8HEneirgr8M1jEAapzXGDqRF",
    "/dns4/node-02.ac-cn-hongkong-c.logos.test.status.im/tcp/30303/p2p/16Uiu2HAm28CoBZjpyxsanC8tQpbvZ7bZJnVYuB1EgFzb571qpWsV",
  ],
};

function assertFleet(mod, name, expected) {
  assert.equal(mod.FLEET_NAME, name);
  assert.deepEqual(mod.NETWORK_CONFIG, expected.networkConfig);
  assert.deepEqual(mod.FLEET_TCP_PEERS, expected.tcpPeers);
  // The parameter pair that decides whether anyone ever sees the offer.
  assert.equal(pubsubTopic(mod.NETWORK_CONFIG), expected.pubsubTopic);
}

test("SWAP_FLEET unset publishes on logos.dev — today's behaviour, unchanged", async () => {
  assertFleet(await importWithFleet(undefined), "logos.dev", LOGOS_DEV);
});

test("SWAP_FLEET=logos.dev is identical to leaving it unset", async () => {
  const unset = await importWithFleet(undefined);
  const explicit = await importWithFleet("logos.dev");
  assert.equal(explicit.FLEET_NAME, unset.FLEET_NAME);
  assert.deepEqual(explicit.NETWORK_CONFIG, unset.NETWORK_CONFIG);
  assert.deepEqual(explicit.FLEET_TCP_PEERS, unset.FLEET_TCP_PEERS);
  assert.equal(explicit.OFFERS_CONTENT_TOPIC, unset.OFFERS_CONTENT_TOPIC);
  assert.equal(explicit.OFFER_REQUESTS_CONTENT_TOPIC, unset.OFFER_REQUESTS_CONTENT_TOPIC);
  assertFleet(explicit, "logos.dev", LOGOS_DEV);
});

test("SWAP_FLEET=logos.test publishes on PR #125's fleet", async () => {
  assertFleet(await importWithFleet("logos.test"), "logos.test", LOGOS_TEST);
});

test("content topics are the same on both fleets (only cluster + peers move)", async () => {
  for (const fleet of [undefined, "logos.dev", "logos.test"]) {
    const mod = await importWithFleet(fleet);
    assert.equal(mod.OFFERS_CONTENT_TOPIC, "/atomic-swaps/1/offers/json");
    assert.equal(mod.OFFER_REQUESTS_CONTENT_TOPIC, "/atomic-swaps/1/offer-requests/json");
  }
});

test("every peer is a raw-TCP :30303 multiaddr on its own fleet's domain", async () => {
  for (const [fleet, domain] of [["logos.dev", "logos.dev.status.im"], ["logos.test", "logos.test.status.im"]]) {
    const mod = await importWithFleet(fleet);
    assert.equal(mod.FLEET_TCP_PEERS.length, 6, `${fleet}: 6 entry nodes`);
    assert.equal(new Set(mod.FLEET_TCP_PEERS).size, 6, `${fleet}: no duplicate peers`);
    for (const addr of mod.FLEET_TCP_PEERS) {
      assert.match(addr, /^\/dns4\/[^/]+\/tcp\/30303\/p2p\/16Uiu2H[1-9A-HJ-NP-Za-km-z]+$/, addr);
      assert.ok(addr.split("/")[2].endsWith(domain), `${addr} is not on ${domain}`);
    }
  }
});

// The whole point of the knob: a wrong value must stop the publisher, not
// quietly publish where nobody is listening.
test("an unknown SWAP_FLEET fails loudly at import, with no fallback", async () => {
  const bad = [
    "",              // set but empty — a broken env file, not an unset var
    " ",
    "logos-test",    // hyphen instead of dot
    "logos.Test",    // wrong case
    "logos.prod",    // a fleet we do not have a table for
    "logos.test ; rm -rf /",
    "true",
    "1",
  ];
  for (const value of bad) {
    await assert.rejects(
      () => importWithFleet(value),
      (err) => {
        assert.match(err.message, /SWAP_FLEET/);
        assert.match(err.message, /logos\.dev, logos\.test/); // names the valid values
        return true;
      },
      `SWAP_FLEET=${JSON.stringify(value)} should have thrown`
    );
  }
});

test("resolveFleetName is the single decision point, and it never guesses", async () => {
  const { resolveFleetName, DEFAULT_FLEET, KNOWN_FLEETS } = await importWithFleet(undefined);
  assert.equal(DEFAULT_FLEET, "logos.dev");
  assert.deepEqual(KNOWN_FLEETS, ["logos.dev", "logos.test"]);
  assert.equal(resolveFleetName(undefined), "logos.dev");
  assert.equal(resolveFleetName("logos.dev"), "logos.dev");
  assert.equal(resolveFleetName("logos.test"), "logos.test");
  // Surrounding whitespace is tolerated (docker env files pick it up easily);
  // anything else throws.
  assert.equal(resolveFleetName("  logos.test\n"), "logos.test");
  assert.throws(() => resolveFleetName("logos.dev2"), /not a known fleet/);
  assert.throws(() => resolveFleetName(""), /not a known fleet/);
});
