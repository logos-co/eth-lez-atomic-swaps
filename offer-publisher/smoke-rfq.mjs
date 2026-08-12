#!/usr/bin/env node
// Live RFQ smoke test against the logos.dev fleet. Proves the on-demand model
// end-to-end with two LIGHT nodes (lightpush + filter only, no relay), exactly
// the maker/taker topology in production:
//
//   * maker node  : filter-subscribes to the offer-requests topic and, on a
//                   valid request, lightpushes an offer (tagged with a unique
//                   nonce) — coalesced to one response per cooldown window.
//   * taker node  : filter-subscribes to the offers topic and lightpushes
//                   offer-requests.
//
// Asserts three things the PR claims:
//   1. a LIGHT node CAN filter-subscribe on the requests topic (the maker
//      receives the request at all) — the key light-node finding;
//   2. the maker responds within ~1 round-trip (latency printed);
//   3. a burst of requests inside the cooldown is COALESCED to one response.
//
// Usage: node smoke-rfq.mjs         (exit 0 = all three verified)

import { createDecoder, createEncoder, utf8ToBytes, bytesToUtf8, waitForRemotePeer, Protocols } from "@waku/sdk";
import { createRoutingInfo } from "@waku/utils";
import {
  OFFERS_CONTENT_TOPIC,
  OFFER_REQUESTS_CONTENT_TOPIC,
  NETWORK_CONFIG,
  createFleetNode,
  dialFleet,
} from "./fleet.mjs";
import { buildOfferRequest, parseOfferRequest, createResponderGate } from "./rfq.mjs";

const log = (msg) => console.log(`[smoke-rfq] ${msg}`);
const fail = (msg) => {
  console.error(`[smoke-rfq] FAIL: ${msg}`);
  process.exit(1);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const timeout = (ms, label) =>
  new Promise((_, reject) =>
    setTimeout(() => reject(new Error(`timeout after ${ms}ms: ${label}`)), ms)
  );

const COOLDOWN_MS = 3000;
const NONCE = `rfq-smoke-${Date.now()}-${Math.random().toString(36).slice(2)}`;

const offersRouting = createRoutingInfo(NETWORK_CONFIG, { contentTopic: OFFERS_CONTENT_TOPIC });
const requestsRouting = createRoutingInfo(NETWORK_CONFIG, { contentTopic: OFFER_REQUESTS_CONTENT_TOPIC });
const offersDecoder = createDecoder(OFFERS_CONTENT_TOPIC, offersRouting);
const offersEncoder = createEncoder({ contentTopic: OFFERS_CONTENT_TOPIC, routingInfo: offersRouting });
const requestsDecoder = createDecoder(OFFER_REQUESTS_CONTENT_TOPIC, requestsRouting);
const requestsEncoder = createEncoder({ contentTopic: OFFER_REQUESTS_CONTENT_TOPIC, routingInfo: requestsRouting });

log(`requests topic ${OFFER_REQUESTS_CONTENT_TOPIC} -> pubsub ${requestsRouting.pubsubTopic}`);

async function connect(label) {
  const node = await createFleetNode();
  const dialed = await dialFleet(node);
  if (dialed === 0) fail(`${label}: could not dial any fleet peer`);
  await Promise.race([
    waitForRemotePeer(node, [Protocols.Filter, Protocols.LightPush]),
    timeout(30_000, `${label}: waiting for filter+lightpush peers`),
  ]);
  log(`${label}: light node up (${dialed}/6 peers, filter+lightpush ready)`);
  return node;
}

const maker = await connect("maker");
const taker = await connect("taker");

// --- Maker: subscribe to requests, respond (coalesced) on the offers topic ---
const gate = createResponderGate(COOLDOWN_MS);
let requestsSeen = 0;
let offersPublished = 0;

function makerOffer() {
  return {
    hashlock: "",
    lez_amount: "1",
    eth_amount: "1",
    maker_eth_address: "0x0000000000000000000000000000000000000000",
    maker_lez_account: NONCE, // nonce so the taker counts only OUR responses
    lez_timelock: Math.floor(Date.now() / 1000) + 300,
    eth_timelock: Math.floor(Date.now() / 1000) + 600,
    lez_htlc_program_id: "0".repeat(64),
    eth_htlc_address: "0x0000000000000000000000000000000000000000",
  };
}

const makerSub = await maker.filter.subscribe(requestsDecoder, async (msg) => {
  const req = parseOfferRequest(msg?.payload);
  if (!req) return;
  requestsSeen += 1;
  if (!gate.tryAcquire()) return; // coalesced
  try {
    await maker.lightPush.send(offersEncoder, {
      payload: utf8ToBytes(JSON.stringify(makerOffer())),
    });
    offersPublished += 1;
    log(`maker: responded to request #${requestsSeen} (publish #${offersPublished})`);
  } catch (err) {
    log(`maker: publish error ${err.message}`);
  }
});
if (makerSub === false) {
  fail("a LIGHT node could NOT filter-subscribe on the requests topic (returned false)");
}
log("maker: filter-subscribed on the requests topic (light node OK)");

// --- Taker: count only responses tagged with our nonce ----------------------
let responsesSeen = 0;
let resolveFirst;
const firstResponse = new Promise((r) => (resolveFirst = r));
await taker.filter.subscribe(offersDecoder, (msg) => {
  try {
    const json = JSON.parse(bytesToUtf8(msg.payload));
    if (json.maker_lez_account === NONCE) {
      responsesSeen += 1;
      resolveFirst();
    }
  } catch {
    /* ignore non-JSON ambient traffic */
  }
});
log("taker: filter-subscribed on the offers topic");

// Give the subscriptions a moment to propagate through the fleet mesh.
await sleep(2000);

async function publishRequest() {
  await taker.lightPush.send(requestsEncoder, {
    payload: utf8ToBytes(JSON.stringify(buildOfferRequest())),
  });
}

// --- 1) round-trip: one request -> one response, measure latency ------------
const t0 = Date.now();
await publishRequest();
try {
  await Promise.race([firstResponse, timeout(30_000, "maker RFQ response")]);
} catch (err) {
  fail(err.message);
}
const latencyMs = Date.now() - t0;
log(`round-trip OK: maker responded in ${latencyMs}ms`);

// --- 2) coalescing: a burst inside the cooldown -> exactly one publish -------
// Wait out the window armed by the round-trip response, then fire a burst.
await sleep(COOLDOWN_MS + 500);
const publishesBeforeBurst = offersPublished;
log("coalescing: firing a burst of 12 requests inside one cooldown window...");
for (let i = 0; i < 12; i++) {
  await publishRequest();
  await sleep(50);
}
await sleep(2000); // let the responses (if any) settle
const burstPublishes = offersPublished - publishesBeforeBurst;
log(`coalescing: 12 requests -> ${burstPublishes} maker publish(es) (requestsSeen=${requestsSeen})`);
if (burstPublishes !== 1) {
  fail(`expected exactly 1 coalesced response to the burst, got ${burstPublishes}`);
}

log(`PASS (latency ${latencyMs}ms, burst 12->1, light-node filter-subscribe OK)`);
if (typeof makerSub === "function") {
  try { await makerSub(); } catch { /* best-effort */ }
}
await maker.stop();
await taker.stop();
process.exit(0);
