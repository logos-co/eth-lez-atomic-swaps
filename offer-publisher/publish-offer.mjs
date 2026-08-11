#!/usr/bin/env node
// Heartbeat offer publisher — long-lived sidecar spawned by
// `swap-cli maker --loop`. Connects once to the logos.test fleet (TCP, Node.js)
// and republishes the maker's offer every OFFER_HEARTBEAT_SECS seconds with
// fresh absolute timelocks. The fleet runs store=false, so republishing is
// what makes the offer visible to late-joining board viewers.
//
// Configuration (environment, all set by swap-cli):
//   OFFER_LEZ_AMOUNT             LEZ sold per swap (integer string)
//   OFFER_ETH_AMOUNT_WEI         price in wei (integer string)
//   OFFER_MAKER_ETH_ADDRESS      0x... address receiving the ETH
//   OFFER_MAKER_LEZ_ACCOUNT      base58 maker LEZ account
//   OFFER_LEZ_TIMELOCK_MINUTES   short (maker) timelock duration
//   OFFER_ETH_TIMELOCK_MINUTES   long (taker) timelock duration
//   OFFER_LEZ_HTLC_PROGRAM_ID    64-hex LEZ HTLC program id
//   OFFER_ETH_HTLC_ADDRESS       0x... EthHTLC contract address
//   OFFER_HEARTBEAT_SECS         republish interval (default 45)

import { createEncoder, utf8ToBytes } from "@waku/sdk";
import { createRoutingInfo } from "@waku/utils";
import {
  OFFERS_CONTENT_TOPIC,
  NETWORK_CONFIG,
  createFleetNode,
  dialFleet,
} from "./fleet.mjs";

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
  heartbeatSecs: Number(process.env.OFFER_HEARTBEAT_SECS || "45"),
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

log(`connecting to logos.test fleet (heartbeat ${cfg.heartbeatSecs}s)...`);
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
const interval = setInterval(publish, cfg.heartbeatSecs * 1000);

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, async () => {
    clearInterval(interval);
    log(`${signal} received, stopping`);
    try {
      await node.stop();
    } finally {
      process.exit(0);
    }
  });
}
