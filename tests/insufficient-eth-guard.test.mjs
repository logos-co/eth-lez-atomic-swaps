// Unit test for the pre-flight ETH funds guard (fix/insufficient-eth-guard).
//
// The live 0.4.4 incident: a taker whose header read "0 ETH" could still
// click Buy on a Market offer. The swap started, and step 2 "Lock your ETH"
// failed with raw JSON-RPC text ("error code -32000: ... insufficient funds
// for gas * price + value ...") shown verbatim as the headline. Two layers
// now stop that:
//
//   1. OfferBoard.qml's hasEnoughEth() — Buy is disabled (with a plain
//      "get test ETH on the Setup tab" reason) when the KNOWN balance can't
//      cover the offer plus a gas margin. Tested here, evaluated straight
//      out of the shipped QML (brace-matched, not hand-copied).
//   2. SwapCopy/Copy.qml's friendlyError() — if an insufficient-funds error
//      still reaches the UI mid-swap, the headline becomes plain language
//      (the raw error stays in the result JSON / receipt journal).
//
// Both layers have C++ twins in swap-ui/src/eth_funds_guard.h (exercised by
// swap-ui/tests/eth_funds_guard_test.cpp). This file additionally pins the
// QML and C++ sides together: the gas-headroom constants and the friendly
// sentence must be identical in both, or the two guards drift apart.
//
// Run: node tests/insufficient-eth-guard.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));

function source(...parts) {
  return readFileSync(join(here, "..", ...parts), "utf8");
}

const offerBoard = source("swap-ui", "src", "qml", "OfferBoard.qml");
const copyQml = source("swap-ui", "src", "qml", "SwapCopy", "Copy.qml");
const guardHeader = source("swap-ui", "src", "eth_funds_guard.h");

function extractFunction(src, name, file) {
  const startRe = new RegExp(`function\\s+${name}\\s*\\([^)]*\\)\\s*\\{`);
  const startMatch = src.match(startRe);
  assert.ok(startMatch, `function ${name} not found in ${file}`);
  let i = startMatch.index + startMatch[0].length;
  let depth = 1;
  while (depth > 0 && i < src.length) {
    if (src[i] === "{") depth += 1;
    else if (src[i] === "}") depth -= 1;
    i += 1;
  }
  return src.slice(startMatch.index, i);
}

const hasEnoughEthSrc = extractFunction(offerBoard, "hasEnoughEth", "OfferBoard.qml");
const hasEnoughEth = new Function(`${hasEnoughEthSrc}; return hasEnoughEth;`)();
const friendlyError = new Function(
  `${extractFunction(copyQml, "friendlyError", "Copy.qml")}; return friendlyError;`,
)();

// The offer from the live incident: 10 LEZ for 0.00001 ETH (1e13 wei).
const offerWei = "10000000000000";

test("a known 0 balance blocks the buy", () => {
  assert.equal(hasEnoughEth("0", offerWei), false);
});

test("an unknown balance never blocks (still loading / fetch failed)", () => {
  assert.equal(hasEnoughEth("", offerWei), true);
  assert.equal(hasEnoughEth(undefined, offerWei), true);
  assert.equal(hasEnoughEth(null, offerWei), true);
  assert.equal(hasEnoughEth("Fetching balances...", offerWei), true);
});

test("the offer amount alone is not enough — gas needs paying too", () => {
  assert.equal(hasEnoughEth(offerWei, offerWei), false);
});

test("offer + headroom is the threshold", () => {
  const headroom = Number(hasEnoughEthSrc.match(/headroomWei = (\d+)/)[1]);
  assert.equal(hasEnoughEth(String(Number(offerWei) + headroom), offerWei), true);
});

test("a funded wallet passes", () => {
  assert.equal(hasEnoughEth("1000000000000000000", offerWei), true); // 1 ETH
});

test("an unreadable offer amount never blocks", () => {
  assert.equal(hasEnoughEth("1000000000000000000", "not-a-number"), true);
});

test("QML gas headroom matches the C++ guard's kEthGasHeadroomWei", () => {
  const qml = hasEnoughEthSrc.match(/headroomWei = (\d+)/);
  assert.ok(qml, "hasEnoughEth lost its inline headroomWei constant");
  const cpp = guardHeader.match(/kEthGasHeadroomWei = "(\d+)"/);
  assert.ok(cpp, "eth_funds_guard.h lost kEthGasHeadroomWei");
  assert.equal(qml[1], cpp[1],
    "OfferBoard.qml and eth_funds_guard.h disagree about the gas headroom");
});

test("insufficient-funds RPC text maps to the plain-language headline", () => {
  const raw =
    "Ethereum RPC error: server returned an error response: error code " +
    "-32000: failed with 16777216 gas: insufficient funds for gas * price " +
    "+ value: address 0x8019...e75B have 0 want 10000000000000";
  const friendly = friendlyError(raw);
  assert.notEqual(friendly, raw);
  assert.doesNotMatch(friendly, /-32000|RPC|0x8019/);
  assert.match(friendly, /ETH balance is too low/);
  assert.match(friendly, /Setup tab/);
});

test("detection is case-insensitive", () => {
  assert.match(friendlyError("INSUFFICIENT FUNDS for gas"), /too low/);
});

test("unrelated errors pass through untouched", () => {
  const raw = "Ethereum RPC error: connection refused";
  assert.equal(friendlyError(raw), raw);
  assert.equal(friendlyError(""), "");
});

test("QML friendly sentence matches the C++ kInsufficientEthDisplayCopy", () => {
  const startMark = "kInsufficientEthDisplayCopy =";
  const start = guardHeader.indexOf(startMark);
  assert.notEqual(start, -1, "eth_funds_guard.h lost kInsufficientEthDisplayCopy");
  const literal = guardHeader.slice(start, guardHeader.indexOf(";", start));
  const cppSentence = [...literal.matchAll(/"([^"]*)"/g)]
    .map((m) => m[1])
    .join("");
  assert.equal(friendlyError("insufficient funds"), cppSentence,
    "Copy.qml's friendlyError and eth_funds_guard.h show different sentences");
});
