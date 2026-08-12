#!/usr/bin/env node
// Adversarial unit test for the offer-board safety filter
// (swap-ui/src/qml/OfferFilter.js), the receive/render-side layer of the
// two-layer venue-trust model (feat/safe-offer-board). The SAME file the QML
// board imports is evaluated here, so this exercises the shipped logic, not a
// copy.
//
// Covers: a non-canonical-venue offer becomes a GHOST (blocked) row (not
// dropped, not acceptable); a malformed offer produces NO row; an honest offer
// is still shown; the ghost cap and the honest spam cap are enforced.
//
// Run: node tests/offer-filter.test.mjs

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import assert from "node:assert";

const here = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(
  join(here, "..", "swap-ui", "src", "qml", "OfferFilter.js"),
  "utf8",
);
// Evaluate the real file and pull out its CommonJS exports.
const factory = new Function("module", src + "\n;return module.exports;");
const F = factory({ exports: {} });

// Canonical pinned venue (shape only; the guard is value-agnostic).
const CANON_ETH = "0x351B0EA07739FA9F6769213927D7836a790A5FAF";
const CANON_LEZ =
  "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

const NOW = 1_000_000;
let seq = 0;
function offer(overrides) {
  seq += 1;
  return Object.assign(
    {
      hashlock: ("" + seq).padStart(64, "0"),
      lez_amount: "150",
      eth_amount: "200000000000000",
      maker_eth_address: "0x351B0EA07739FA9F6769213927D7836a790A5FAF",
      maker_lez_account: "lez1abc",
      lez_timelock: NOW + 2000,
      eth_timelock: NOW + 2600,
      lez_htlc_program_id: CANON_LEZ,
      eth_htlc_address: CANON_ETH,
    },
    overrides || {},
  );
}

function ctx(overrides) {
  return Object.assign(
    {
      nowSec: NOW,
      canonicalEth: CANON_ETH,
      canonicalLez: CANON_LEZ,
      existingKeys: [],
      ghostCount: 0,
      honestCount: 0,
      maxOffers: 200,
      maxGhostRows: 4,
      keyOf: (o) => o.hashlock,
    },
    overrides || {},
  );
}

let failures = 0;
function check(label, fn) {
  try {
    fn();
    console.log("ok   " + label);
  } catch (e) {
    failures += 1;
    console.error("FAIL " + label + "\n     " + e.message);
  }
}

// --- Honest offer: admitted, not blocked. -----------------------------------
check("honest canonical offer is admitted and acceptable", () => {
  const admit = F.classifyOffers([offer()], ctx());
  assert.equal(admit.length, 1);
  assert.equal(admit[0].blocked, false);
});

// --- Venue-mismatch: GHOST (admitted + blocked), NOT dropped. ----------------
check("non-canonical ETH contract becomes a ghost (blocked) row", () => {
  const admit = F.classifyOffers(
    [offer({ eth_htlc_address: "0xdeadBEEFdeadBEEFdeadBEEFdeadBEEFdeadBEEF" })],
    ctx(),
  );
  assert.equal(admit.length, 1, "ghost is rendered, not dropped");
  assert.equal(admit[0].blocked, true, "and marked blocked (not acceptable)");
});

check("non-canonical LEZ program becomes a ghost (blocked) row", () => {
  const admit = F.classifyOffers(
    [offer({ lez_htlc_program_id: "ff".repeat(32) })],
    ctx(),
  );
  assert.equal(admit.length, 1);
  assert.equal(admit[0].blocked, true);
});

// --- Malformed: NO row (silent drop). ---------------------------------------
check("malformed offer (bad hex address) produces no row", () => {
  const admit = F.classifyOffers(
    [offer({ maker_eth_address: "not-an-address" })],
    ctx(),
  );
  assert.equal(admit.length, 0);
});

check("NaN timelock produces no row (un-prunable-spam guard)", () => {
  const admit = F.classifyOffers([offer({ lez_timelock: "NaN" })], ctx());
  assert.equal(admit.length, 0);
});

check("zero / negative timelock produces no row", () => {
  assert.equal(F.classifyOffers([offer({ eth_timelock: 0 })], ctx()).length, 0);
  assert.equal(
    F.classifyOffers([offer({ lez_timelock: -5 })], ctx()).length,
    0,
  );
});

check("non-positive amount produces no row", () => {
  assert.equal(F.classifyOffers([offer({ lez_amount: "0" })], ctx()).length, 0);
  assert.equal(
    F.classifyOffers([offer({ eth_amount: "junk" })], ctx()).length,
    0,
  );
});

check("already-expired offer produces no row", () => {
  const admit = F.classifyOffers(
    [offer({ lez_timelock: NOW - 1, eth_timelock: NOW - 1 })],
    ctx(),
  );
  assert.equal(admit.length, 0);
});

check("duplicate of an existing key is dropped", () => {
  const o = offer();
  const admit = F.classifyOffers([o], ctx({ existingKeys: [o.hashlock] }));
  assert.equal(admit.length, 0);
});

// --- Ghost cap: at most maxGhostRows blocked rows admitted. ------------------
check("ghost cap: a flood of venue-mismatch offers is capped", () => {
  const flood = [];
  for (let i = 0; i < 10; i++)
    flood.push(offer({ eth_htlc_address: "0x" + "ab".repeat(20) }));
  const admit = F.classifyOffers(flood, ctx({ maxGhostRows: 4 }));
  assert.equal(admit.length, 4, "only maxGhostRows ghosts admitted");
  assert.ok(admit.every((a) => a.blocked));
});

check("ghost cap counts ghosts already on the board", () => {
  const flood = [];
  for (let i = 0; i < 10; i++)
    flood.push(offer({ eth_htlc_address: "0x" + "cd".repeat(20) }));
  const admit = F.classifyOffers(flood, ctx({ maxGhostRows: 4, ghostCount: 3 }));
  assert.equal(admit.length, 1, "only room for one more ghost");
});

// --- Spam cap for honest offers. --------------------------------------------
check("honest spam cap is enforced", () => {
  const many = [];
  for (let i = 0; i < 6; i++) many.push(offer());
  const admit = F.classifyOffers(many, ctx({ maxOffers: 3 }));
  assert.equal(admit.length, 3);
  assert.ok(admit.every((a) => !a.blocked));
});

// --- Unknown canonical (startup): do NOT ghost. -----------------------------
check("unresolved canonical venue does not ghost (no false positives)", () => {
  const admit = F.classifyOffers(
    [offer({ eth_htlc_address: "0x" + "ef".repeat(20) })],
    ctx({ canonicalEth: "", canonicalLez: "" }),
  );
  assert.equal(admit.length, 1);
  assert.equal(admit[0].blocked, false);
});

// --- hex normalisation parity with the accept-time check. -------------------
check("venue match is case- and 0x-insensitive", () => {
  const admit = F.classifyOffers(
    [
      offer({
        eth_htlc_address: CANON_ETH.toLowerCase(),
        lez_htlc_program_id: "0x" + CANON_LEZ,
      }),
    ],
    ctx(),
  );
  assert.equal(admit.length, 1);
  assert.equal(admit[0].blocked, false);
});

if (failures > 0) {
  console.error(`\n${failures} assertion(s) FAILED`);
  process.exit(1);
}
console.log("\nALL PASSED");
