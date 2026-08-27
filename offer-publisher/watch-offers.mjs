#!/usr/bin/env node
// External liveness check: subscribes to /atomic-swaps/1/offers/json on the
// fleet selected by SWAP_FLEET (default logos.dev; SWAP_FLEET=logos.test
// watches the migration fleet) and prints each offer as it arrives, with its
// age since the offer's own `lez_timelock`/`eth_timelock` fields imply it was
// minted.
//
// Run this from OUTSIDE the machine hosting the maker (your laptop, not the
// VPS) — that is the actual proof the fleet is relaying the maker's
// heartbeat to a third party, not just that the maker process is alive.
// The fleet runs store=false: nothing is retained, so an offer this script
// never sees never reached anyone either.
//
// Usage: [SWAP_FLEET=logos.test] node watch-offers.mjs [--timeout-secs N]
//   (no timeout by default — Ctrl-C to stop)

import { createDecoder, bytesToUtf8 } from "@waku/sdk";
import { createRoutingInfo } from "@waku/utils";
import {
  OFFERS_CONTENT_TOPIC,
  NETWORK_CONFIG,
  FLEET_NAME,
  createFleetNode,
  dialFleet,
} from "./fleet.mjs";

const log = (msg) => console.log(`[watch-offers] ${msg}`);

const timeoutArgIdx = process.argv.indexOf("--timeout-secs");
const timeoutSecs =
  timeoutArgIdx !== -1 ? Number(process.argv[timeoutArgIdx + 1]) : null;

const routingInfo = createRoutingInfo(NETWORK_CONFIG, { contentTopic: OFFERS_CONTENT_TOPIC });
log(`fleet ${FLEET_NAME}`);
log(`content topic ${OFFERS_CONTENT_TOPIC}`);
log(
  `cluster ${NETWORK_CONFIG.clusterId}, ${NETWORK_CONFIG.numShardsInCluster} shards -> pubsub topic ${routingInfo.pubsubTopic}`
);

log("creating light node (TCP transport)...");
const node = await createFleetNode();
log(`peer id ${node.libp2p.peerId.toString()}`);

const dialed = await dialFleet(node, log);
if (dialed === 0) {
  console.error("[watch-offers] could not dial any of the 6 fleet peers");
  process.exit(1);
}
log(`dialed ${dialed}/6 fleet peers`);

const decoder = createDecoder(OFFERS_CONTENT_TOPIC, routingInfo);

let count = 0;
const firstSeenAt = new Map(); // maker_lez_account -> Date.now() of first sighting, for a simple "age since we started watching"
const startedAt = Date.now();

const subscribed = await node.filter.subscribe(decoder, (msg) => {
  count += 1;
  const receivedAt = Date.now();
  let offer;
  try {
    offer = JSON.parse(bytesToUtf8(msg.payload));
  } catch {
    log(`#${count}: non-JSON payload (${msg.payload.length} bytes) — ignoring`);
    return;
  }

  const maker = offer.maker_lez_account ?? "?";
  if (!firstSeenAt.has(maker)) firstSeenAt.set(maker, receivedAt);
  const sinceFirstSeen = ((receivedAt - firstSeenAt.get(maker)) / 1000).toFixed(1);
  const sinceWatchStart = ((receivedAt - startedAt) / 1000).toFixed(1);

  const now = Math.floor(receivedAt / 1000);
  const lezExpiresIn = offer.lez_timelock ? offer.lez_timelock - now : "?";
  const ethExpiresIn = offer.eth_timelock ? offer.eth_timelock - now : "?";

  log(
    `#${count} t+${sinceWatchStart}s (maker seen ${sinceFirstSeen}s ago) — ` +
      `${offer.lez_amount ?? "?"} LEZ -> ${offer.eth_amount ?? "?"} wei | ` +
      `maker_lez_account=${maker} maker_eth_address=${offer.maker_eth_address ?? "?"} | ` +
      `lez_timelock expires in ${lezExpiresIn}s, eth_timelock expires in ${ethExpiresIn}s`
  );
});

if (!subscribed) {
  console.error("[watch-offers] filter subscribe returned false");
  process.exit(1);
}
log("subscribed — waiting for offers (Ctrl-C to stop)...");

async function shutdown(code) {
  log(`stopping after ${count} offer(s) seen`);
  await node.stop();
  process.exit(code);
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => shutdown(0));
}

if (timeoutSecs) {
  setTimeout(() => {
    log(`--timeout-secs ${timeoutSecs} reached`);
    shutdown(count > 0 ? 0 : 1);
  }, timeoutSecs * 1000);
}
