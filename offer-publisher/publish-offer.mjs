#!/usr/bin/env node
// Offer publisher — long-lived sidecar spawned by `swap-cli maker --loop`.
// Connects once to the delivery fleet selected by SWAP_FLEET (TCP, Node.js;
// default logos.dev — see fleet.mjs) and makes the maker's offer visible two
// ways (the fleet runs store=false, so an offer is only visible while it is
// being (re)published):
//
//  1. RFQ responder (the accelerator): filter-subscribes to the anonymous
//     offer-requests topic and, when a taker asks, publishes the current offer
//     IMMEDIATELY — but COALESCED to at most one response per
//     RESPONSE_COOLDOWN_SECS regardless of request volume (anti-amplification:
//     the requests topic is unauthenticated, so 100 pings in a window -> 1
//     offer). Malformed requests are ignored. The responder is failure-
//     isolated: any error in it never stops the fallback heartbeat below.
//
//  2. Fallback heartbeat (the reliable baseline): republishes the offer every
//     FALLBACK_HEARTBEAT_SECS with fresh absolute timelocks, so a taker who
//     missed the RFQ response (or whose request the maker coalesced away) still
//     sees an offer. If a light node cannot filter-subscribe on this fleet, the
//     heartbeat alone keeps the board filling — just without the instant fill.
//
// Configuration (environment, OFFER_* set by swap-cli; the RFQ knobs are read
// from the inherited process env — see deploy/maker.env.example):
//   OFFER_LEZ_AMOUNT             LEZ sold per swap (integer string)
//   OFFER_ETH_AMOUNT_WEI         price in wei (integer string)
//   OFFER_MAKER_ETH_ADDRESS      0x... address receiving the ETH
//   OFFER_MAKER_LEZ_ACCOUNT      base58 maker LEZ account
//   OFFER_LEZ_TIMELOCK_MINUTES   short (maker) timelock duration
//   OFFER_ETH_TIMELOCK_MINUTES   long (taker) timelock duration
//   OFFER_LEZ_HTLC_PROGRAM_ID    64-hex LEZ HTLC program id
//   OFFER_ETH_HTLC_ADDRESS       0x... EthHTLC contract address
//   FALLBACK_HEARTBEAT_SECS      fallback republish interval (default 30;
//                                falls back to the legacy OFFER_HEARTBEAT_SECS
//                                that swap-cli still sets, else 30)
//   RESPONSE_COOLDOWN_SECS       min seconds between RFQ responses (default 8)
//   SWAP_FLEET                   which fleet to publish on: unset/logos.dev
//                                (default) or logos.test. An unknown value
//                                aborts at startup — see fleet.mjs. One
//                                publisher serves ONE fleet; dual-homing a
//                                maker means running one per fleet.

import { createDecoder, createEncoder, utf8ToBytes } from "@waku/sdk";
import { createRoutingInfo } from "@waku/utils";
import {
  OFFERS_CONTENT_TOPIC,
  OFFER_REQUESTS_CONTENT_TOPIC,
  NETWORK_CONFIG,
  FLEET_NAME,
  createFleetNode,
  dialFleet,
} from "./fleet.mjs";
import { parseOfferRequest, createResponderGate } from "./rfq.mjs";

const log = (msg) => console.log(`[offer-publisher] ${msg}`);

function requireEnv(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`[offer-publisher] missing required env var ${name}`);
    process.exit(2);
  }
  return value;
}

const cfg = {
  lezAmount: requireEnv("OFFER_LEZ_AMOUNT"),
  ethAmountWei: requireEnv("OFFER_ETH_AMOUNT_WEI"),
  makerEthAddress: requireEnv("OFFER_MAKER_ETH_ADDRESS"),
  makerLezAccount: requireEnv("OFFER_MAKER_LEZ_ACCOUNT"),
  lezTimelockMinutes: Number(requireEnv("OFFER_LEZ_TIMELOCK_MINUTES")),
  ethTimelockMinutes: Number(requireEnv("OFFER_ETH_TIMELOCK_MINUTES")),
  lezHtlcProgramId: requireEnv("OFFER_LEZ_HTLC_PROGRAM_ID"),
  ethHtlcAddress: requireEnv("OFFER_ETH_HTLC_ADDRESS"),
  // The fallback heartbeat supersedes the old 5s/45s heartbeat: RFQ is now the
  // fast path, this is the slow reliable baseline. Prefer the new env name but
  // honour the legacy OFFER_HEARTBEAT_SECS that swap-cli still injects so
  // deployed makers keep a sane interval without a swap-cli change.
  fallbackHeartbeatSecs: Number(
    process.env.FALLBACK_HEARTBEAT_SECS || process.env.OFFER_HEARTBEAT_SECS || "30"
  ),
  responseCooldownSecs: Number(process.env.RESPONSE_COOLDOWN_SECS || "8"),
};

// Offer schema per swap-module/src/swap_delivery_adapter.cpp (offerKeys):
// hashlock is empty in offers — takers generate the preimage after discovery.
function buildOffer() {
  const now = Math.floor(Date.now() / 1000);
  return {
    hashlock: "",
    lez_amount: cfg.lezAmount,
    eth_amount: cfg.ethAmountWei,
    maker_eth_address: cfg.makerEthAddress,
    maker_lez_account: cfg.makerLezAccount,
    lez_timelock: now + cfg.lezTimelockMinutes * 60,
    eth_timelock: now + cfg.ethTimelockMinutes * 60,
    lez_htlc_program_id: cfg.lezHtlcProgramId,
    eth_htlc_address: cfg.ethHtlcAddress,
  };
}

const routingInfo = createRoutingInfo(NETWORK_CONFIG, {
  contentTopic: OFFERS_CONTENT_TOPIC,
});
const encoder = createEncoder({ contentTopic: OFFERS_CONTENT_TOPIC, routingInfo });

// Name the fleet and its pubsub topic up front: an offer published on the
// wrong fleet looks exactly like a healthy one in every later log line.
log(
  `connecting to ${FLEET_NAME} fleet ` +
    `(cluster ${NETWORK_CONFIG.clusterId} -> ${routingInfo.pubsubTopic}, ` +
    `fallback heartbeat ${cfg.fallbackHeartbeatSecs}s, ` +
    `RFQ response cooldown ${cfg.responseCooldownSecs}s)...`
);
const node = await createFleetNode();
const dialed = await dialFleet(node, log);
if (dialed === 0) {
  console.error("[offer-publisher] could not dial any fleet peer");
  process.exit(1);
}
log(`connected (${dialed}/6 fleet peers)`);

let consecutiveFailures = 0;
const MAX_CONSECUTIVE_FAILURES = 10;

async function publish() {
  const offer = buildOffer();
  try {
    const result = await node.lightPush.send(encoder, {
      payload: utf8ToBytes(JSON.stringify(offer)),
    });
    const successes = result.successes?.length ?? 0;
    if (successes > 0) {
      consecutiveFailures = 0;
      log(
        `offer published (${successes} peer(s)): ${cfg.lezAmount} LEZ -> ${cfg.ethAmountWei} wei`
      );
    } else {
      consecutiveFailures += 1;
      log(`lightpush rejected by all peers (${consecutiveFailures} consecutive failures)`);
    }
  } catch (err) {
    consecutiveFailures += 1;
    log(`publish error: ${err.message} (${consecutiveFailures} consecutive failures)`);
  }
  if (consecutiveFailures >= MAX_CONSECUTIVE_FAILURES) {
    // Exit non-zero: the swap-cli supervisor restarts us with a fresh node.
    console.error("[offer-publisher] too many consecutive failures, exiting for restart");
    process.exit(1);
  }
}

await publish();
const interval = setInterval(publish, cfg.fallbackHeartbeatSecs * 1000);

// --- RFQ responder ---------------------------------------------------------
// Filter-subscribe to the anonymous offer-requests topic and respond by
// publishing the current offer, coalesced to one publish per cooldown window.
// FAILURE-ISOLATED: any error setting up or handling the subscription is
// logged and swallowed so the fallback heartbeat above keeps running — offers
// must still flow if the RFQ path breaks (or if this fleet's light nodes
// cannot filter-subscribe at all).
let unsubscribeRequests = null;
async function startRfqResponder() {
  const gate = createResponderGate(cfg.responseCooldownSecs * 1000);
  const requestRoutingInfo = createRoutingInfo(NETWORK_CONFIG, {
    contentTopic: OFFER_REQUESTS_CONTENT_TOPIC,
  });
  const requestDecoder = createDecoder(
    OFFER_REQUESTS_CONTENT_TOPIC,
    requestRoutingInfo
  );

  const onRequest = (msg) => {
    // Never let a bad message escape into the process: the whole handler is
    // guarded so a malformed payload or a transient publish error can't crash
    // the sidecar and take the heartbeat down with it.
    try {
      const req = parseOfferRequest(msg?.payload);
      if (!req) {
        return; // malformed / not an offer_request — ignore silently
      }
      if (!gate.tryAcquire()) {
        // Coalesced: a response already went out this window. This is the
        // anti-amplification guarantee (100 pings -> 1 offer).
        return;
      }
      log("RFQ: offer-request received -> responding with current offer");
      // Fire-and-forget; publish() has its own try/catch and failure counter.
      publish();
    } catch (err) {
      log(`RFQ responder handler error (ignored): ${err?.message ?? err}`);
    }
  };

  const subscribed = await node.filter.subscribe(requestDecoder, onRequest);
  if (subscribed === false) {
    log(
      "RFQ: filter-subscribe on the requests topic returned false — running " +
        "heartbeat-only (offers still flow via the fallback)"
    );
    return;
  }
  unsubscribeRequests = subscribed;
  log(
    `RFQ responder active on ${OFFER_REQUESTS_CONTENT_TOPIC} ` +
      `(coalesced to 1 response / ${cfg.responseCooldownSecs}s)`
  );
}

try {
  await startRfqResponder();
} catch (err) {
  log(
    `RFQ responder unavailable (ignored, heartbeat continues): ` +
      `${err?.message ?? err}`
  );
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, async () => {
    clearInterval(interval);
    log(`${signal} received, stopping`);
    try {
      if (typeof unsubscribeRequests === "function") {
        await unsubscribeRequests();
      }
    } catch {
      // best-effort teardown
    }
    try {
      await node.stop();
    } finally {
      process.exit(0);
    }
  });
}
