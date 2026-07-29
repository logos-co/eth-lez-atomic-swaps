// Offer board: browser @waku/sdk light node subscribed to the atomic-swaps
// offers topic on the logos.dev fleet (cluster 2, autosharding, 8 shards).
// Read-only: filter-subscribe + render. No keys, no backend.

import { createLightNode, createDecoder, bytesToUtf8, waitForRemotePeer, Protocols } from "@waku/sdk";
import { createRoutingInfo } from "@waku/utils";

const OFFERS_CONTENT_TOPIC = "/atomic-swaps/1/offers/json";
const NETWORK_CONFIG = { clusterId: 2, numShardsInCluster: 8 };

const config = window.OFFER_BOARD_CONFIG ?? {};
const bootstrapPeers = config.bootstrapPeers ?? [];
const heartbeatSecs = config.heartbeatSecs ?? 45;
const ttlMs = (2 * heartbeatSecs + 30) * 1000;

// --- DOM ---
const statusEl = document.getElementById("status");
const statusText = document.getElementById("status-text");
const noticeEl = document.getElementById("notice");
const offersEl = document.getElementById("offers");
const emptyEl = document.getElementById("empty");
const peerCountEl = document.getElementById("peer-count");
const seenCountEl = document.getElementById("seen-count");
document.getElementById("ttl-label").textContent = `${Math.round(ttlMs / 1000)}s`;

function setStatus(state, text) {
  statusEl.className = state;
  statusText.textContent = text;
}

function showNotice(html) {
  noticeEl.innerHTML = html;
  noticeEl.style.display = "block";
}

// --- Offer model ---
// Keyed by maker + terms; refreshed by each heartbeat; expired after ttlMs.
const offers = new Map();
let seenCount = 0;

const REQUIRED_FIELDS = [
  "lez_amount",
  "eth_amount",
  "maker_eth_address",
  "maker_lez_account",
  "lez_timelock",
  "eth_timelock",
  "lez_htlc_program_id",
  "eth_htlc_address",
];

function offerKey(o) {
  return [o.maker_lez_account, o.maker_eth_address, o.lez_amount, o.eth_amount].join("|");
}

function onOfferPayload(payload) {
  let offer;
  try {
    offer = JSON.parse(bytesToUtf8(payload));
  } catch {
    return; // non-JSON payload on the topic — ignore
  }
  if (!REQUIRED_FIELDS.every((f) => offer[f] !== undefined && offer[f] !== "")) return;
  seenCount += 1;
  seenCountEl.textContent = String(seenCount);
  offers.set(offerKey(offer), { offer, lastSeen: Date.now() });
  render();
}

// --- Rendering ---
function weiToEth(wei) {
  try {
    const w = BigInt(wei);
    const whole = w / 10n ** 18n;
    const frac = (w % 10n ** 18n).toString().padStart(18, "0").replace(/0+$/, "");
    return frac ? `${whole}.${frac}` : `${whole}`;
  } catch {
    return String(wei);
  }
}

function truncate(s, head = 8, tail = 6) {
  s = String(s);
  return s.length > head + tail + 1 ? `${s.slice(0, head)}…${s.slice(-tail)}` : s;
}

function ago(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s ago`;
  return `${Math.floor(s / 60)}m ${s % 60}s ago`;
}

function timelockLabel(unixSecs) {
  const mins = Math.round((unixSecs * 1000 - Date.now()) / 60000);
  return mins >= 0 ? `${mins}m` : "expired";
}

function render() {
  const now = Date.now();
  for (const [key, entry] of offers) {
    if (now - entry.lastSeen > ttlMs) offers.delete(key);
  }

  offersEl.innerHTML = "";
  emptyEl.style.display = offers.size === 0 ? "block" : "none";

  const sorted = [...offers.values()].sort((a, b) => b.lastSeen - a.lastSeen);
  for (const { offer, lastSeen } of sorted) {
    const age = now - lastSeen;
    const row = document.createElement("tr");
    row.className = "offer";
    row.innerHTML = `
      <td>LEZ &rarr; ETH</td>
      <td class="amount give">${escapeHtml(offer.lez_amount)} LEZ</td>
      <td class="amount">${escapeHtml(weiToEth(offer.eth_amount))} ETH</td>
      <td class="mono">${escapeHtml(timelockLabel(offer.lez_timelock))} / ${escapeHtml(timelockLabel(offer.eth_timelock))}</td>
      <td class="mono" title="${escapeHtml(offer.maker_eth_address)} / ${escapeHtml(offer.maker_lez_account)}">
        ${escapeHtml(truncate(offer.maker_eth_address))}<br>${escapeHtml(truncate(offer.maker_lez_account))}
      </td>
      <td class="${age < heartbeatSecs * 2000 ? "fresh" : "stale"}">${ago(age)}</td>`;
    offersEl.appendChild(row);
  }
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

setInterval(render, 1000);

// --- Waku ---
async function main() {
  if (bootstrapPeers.length === 0) {
    setStatus("blocked", "no WSS bootstrap");
    showNotice(
      "<strong>Not connected:</strong> the logos.dev delivery fleet exposes only raw TCP " +
        "(<code>/tcp/30303</code>), which browsers cannot dial. Ask the fleet operators for a " +
        "WSS (secure websocket) listener on the cluster-2 delivery nodes and add its multiaddr " +
        "to <code>config.js</code> (<code>bootstrapPeers</code>). Offer flow itself is verified " +
        "working from Node.js — see <code>web/offer-board/smoke.mjs</code>."
    );
    return;
  }

  setStatus("connecting", "connecting…");
  try {
    const node = await createLightNode({
      networkConfig: NETWORK_CONFIG,
      bootstrapPeers,
      discovery: { dns: false, peerExchange: true, peerCache: true },
    });

    const updatePeers = () => {
      const n = node.libp2p.getPeers().length;
      peerCountEl.textContent = String(n);
      if (n > 0) setStatus("connected", `connected (${n} peer${n === 1 ? "" : "s"})`);
      else setStatus("connecting", "searching for peers…");
    };
    setInterval(updatePeers, 2000);

    await waitForRemotePeer(node, [Protocols.Filter], 45000);

    const routingInfo = createRoutingInfo(NETWORK_CONFIG, {
      contentTopic: OFFERS_CONTENT_TOPIC,
    });
    const decoder = createDecoder(OFFERS_CONTENT_TOPIC, routingInfo);
    const ok = await node.filter.subscribe(decoder, (msg) => onOfferPayload(msg.payload));
    if (!ok) {
      setStatus("error", "subscribe failed");
      showNotice("Filter subscription failed — the peer may not serve the offers shard.");
      return;
    }
    updatePeers();
  } catch (err) {
    setStatus("error", "connection failed");
    showNotice(`Connection failed: <code>${escapeHtml(err.message)}</code>`);
  }
}

main();
