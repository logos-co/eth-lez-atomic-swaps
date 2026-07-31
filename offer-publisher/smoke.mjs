#!/usr/bin/env node
// Smoke test: prove Node.js @waku/sdk connectivity to the logos.dev fleet on
// the offers channel. Connects over TCP (browser transport is WSS — see the
// BLOCKED note in README.md), subscribes via filter, publishes a canary offer
// via lightpush, and asserts the canary comes back through the filter
// subscription (full publish -> fleet -> subscribe round-trip).
//
// Usage: node smoke.mjs            (exit 0 = round-trip verified)

import { createDecoder, createEncoder, utf8ToBytes, bytesToUtf8, waitForRemotePeer, Protocols } from "@waku/sdk";
import { createRoutingInfo } from "@waku/utils";
import {
  OFFERS_CONTENT_TOPIC,
  NETWORK_CONFIG,
  createFleetNode,
  dialFleet,
} from "./fleet.mjs";

const log = (msg) => console.log(`[smoke] ${msg}`);
const fail = (msg) => {
  console.error(`[smoke] FAIL: ${msg}`);
  process.exit(1);
};

const timeout = (ms, label) =>
  new Promise((_, reject) =>
    setTimeout(() => reject(new Error(`timeout after ${ms}ms: ${label}`)), ms)
  );

const routingInfo = createRoutingInfo(NETWORK_CONFIG, {
  contentTopic: OFFERS_CONTENT_TOPIC,
});
log(`content topic ${OFFERS_CONTENT_TOPIC}`);
log(`cluster ${NETWORK_CONFIG.clusterId}, ${NETWORK_CONFIG.numShardsInCluster} shards -> pubsub topic ${routingInfo.pubsubTopic}`);

log("creating light node (TCP transport)...");
const node = await createFleetNode();
log(`peer id ${node.libp2p.peerId.toString()}`);

const dialed = await dialFleet(node, log);
if (dialed === 0) fail("could not dial any of the 6 fleet peers");
log(`dialed ${dialed}/6 fleet peers`);

try {
  await Promise.race([
    waitForRemotePeer(node, [Protocols.Filter, Protocols.LightPush]),
    timeout(30_000, "waiting for filter+lightpush peers"),
  ]);
} catch (err) {
  fail(err.message);
}
log("fleet peers support filter + lightpush");

// Subscribe first, then publish a canary and wait for it to come back.
const canaryNonce = `smoke-${Date.now()}-${Math.random().toString(36).slice(2)}`;
const canary = {
  hashlock: "",
  lez_amount: "1",
  eth_amount: "1",
  maker_eth_address: "0x0000000000000000000000000000000000000000",
  maker_lez_account: canaryNonce, // nonce carried in an offer field for matching
  lez_timelock: Math.floor(Date.now() / 1000) + 60,
  eth_timelock: Math.floor(Date.now() / 1000) + 120,
  lez_htlc_program_id: "0".repeat(64),
  eth_htlc_address: "0x0000000000000000000000000000000000000000",
};

const decoder = createDecoder(OFFERS_CONTENT_TOPIC, routingInfo);
const encoder = createEncoder({ contentTopic: OFFERS_CONTENT_TOPIC, routingInfo });

let received = 0;
let resolveRoundTrip;
const roundTrip = new Promise((resolve) => (resolveRoundTrip = resolve));

const subscribed = await node.filter.subscribe(decoder, (msg) => {
  received += 1;
  try {
    const json = JSON.parse(bytesToUtf8(msg.payload));
    log(`filter message #${received}: maker_lez_account=${json.maker_lez_account ?? "?"}`);
    if (json.maker_lez_account === canaryNonce) resolveRoundTrip();
  } catch {
    log(`filter message #${received}: non-JSON payload (${msg.payload.length} bytes)`);
  }
});
if (!subscribed) fail("filter subscribe returned false");
log("filter subscription active on offers topic");

const sendResult = await node.lightPush.send(encoder, {
  payload: utf8ToBytes(JSON.stringify(canary)),
});
const successes = sendResult.successes?.length ?? 0;
if (successes === 0) {
  fail(`lightpush rejected by all peers: ${JSON.stringify(sendResult.failures ?? [])}`);
}
log(`lightpush accepted by ${successes} peer(s)`);

try {
  await Promise.race([roundTrip, timeout(30_000, "canary round-trip via filter")]);
  log("round-trip OK: canary offer published and received back via filter");
} catch (err) {
  fail(err.message);
}

const peers = node.libp2p.getPeers();
log(`connected peers: ${peers.length}`);
log("PASS");
await node.stop();
process.exit(0);
