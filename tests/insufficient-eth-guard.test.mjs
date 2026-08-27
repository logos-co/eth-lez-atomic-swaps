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
//   3. AtomicSwapView.qml's noticeDuplicatesReceipt() — that plain sentence
//      is shown ONCE. handleTakerFinished() writes takerResultJson (the
//      "Your swap" receipt card) and then errorMessage (the shell's error
//      strip) from one backend event, so both surfaces rendered the same
//      failure at the same time. The strip now stands down while the tab
//      owning that receipt is on screen.
//
// Layers 1 and 2 have C++ twins in swap-ui/src/eth_funds_guard.h (exercised by
// swap-ui/tests/eth_funds_guard_test.cpp). This file additionally pins the
// QML and C++ sides together: the gas-headroom constants and the friendly
// sentence must be identical in both, or the two guards drift apart — and
// layer 3 depends on that equality, since it recognises the duplicate by
// comparing the two surfaces' text.
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
const shellQml = source("swap-ui", "src", "qml", "AtomicSwapView.qml");

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
const resultError = new Function(
  `${extractFunction(shellQml, "resultError", "AtomicSwapView.qml")}; return resultError;`,
)();
const noticeDuplicatesReceipt = new Function(
  `${extractFunction(shellQml, "noticeDuplicatesReceipt", "AtomicSwapView.qml")};` +
    " return noticeDuplicatesReceipt;",
)();

// The shell's tab order (AtomicSwapView.qml `tabs`); "swap" is the tab whose
// TakerView owns the taker receipt card.
const SWAP_TAB = 1;
const SETUP_TAB = 5;

// The verbatim text the backend hands the UI on the live incident.
const RAW_INSUFFICIENT =
  "Ethereum RPC error: server returned an error response: error code " +
  "-32000: failed with 16777216 gas: insufficient funds for gas * price " +
  "+ value: address 0x8019...e75B have 0 want 10000000000000";

// What the strip and the card each end up holding for that failure: the C++
// displaySwapError() puts the friendly sentence in errorMessage, and the card
// runs the raw error from takerResultJson through friendlyError().
function stripText() {
  return friendlyError(RAW_INSUFFICIENT);
}
function receiptHeadline(resultJson) {
  return friendlyError(resultError(resultJson));
}

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
  const raw = RAW_INSUFFICIENT;
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

// --- Layer 3: the sentence reaches the user once ------------------------

test("resultError pulls the backend error out of a result JSON", () => {
  assert.equal(
    resultError(JSON.stringify({ error: RAW_INSUFFICIENT })),
    RAW_INSUFFICIENT,
  );
});

test("resultError is empty for a successful swap and for no result at all", () => {
  assert.equal(resultError(JSON.stringify({ status: "completed" })), "");
  assert.equal(resultError(""), "");
});

test("resultError treats unparseable JSON as the error text (as ReceiptCard does)", () => {
  assert.equal(resultError("not json at all"), "not json at all");
});

test("the failed swap shows the SAME sentence on both surfaces", () => {
  // The duplication being deduped. If these ever differ, the strip would stay
  // up next to the card and the user would read two problems again.
  const json = JSON.stringify({ error: RAW_INSUFFICIENT });
  assert.equal(receiptHeadline(json), stripText());
  assert.match(stripText(), /ETH balance is too low/);
});

test("the strip stands down while the receipt card is the tab on screen", () => {
  const json = JSON.stringify({ error: RAW_INSUFFICIENT });
  assert.equal(
    noticeDuplicatesReceipt(stripText(), SWAP_TAB, SWAP_TAB, receiptHeadline(json)),
    true,
  );
});

test("the strip comes back on the tab the copy sends the user to", () => {
  // "Add Sepolia test ETH from the Setup tab and try again" — the receipt card
  // does not follow them there, so the global notice must.
  const json = JSON.stringify({ error: RAW_INSUFFICIENT });
  assert.equal(
    noticeDuplicatesReceipt(stripText(), SETUP_TAB, SWAP_TAB, receiptHeadline(json)),
    false,
  );
});

test("a failure with no receipt keeps the strip — it is the only surface", () => {
  // Start failures (handleJobStartResult) set errorMessage without writing
  // takerResultJson, so there is no card to defer to.
  assert.equal(
    noticeDuplicatesReceipt(stripText(), SWAP_TAB, SWAP_TAB, receiptHeadline("")),
    false,
  );
});

test("a DIFFERENT error from the one on the receipt keeps the strip", () => {
  // A stale receipt from an earlier run must not silence a new failure.
  const stale = JSON.stringify({ error: "Ethereum RPC error: connection refused" });
  assert.equal(
    noticeDuplicatesReceipt(stripText(), SWAP_TAB, SWAP_TAB, receiptHeadline(stale)),
    false,
  );
});

test("no notice, no suppression", () => {
  const json = JSON.stringify({ error: RAW_INSUFFICIENT });
  assert.equal(
    noticeDuplicatesReceipt("", SWAP_TAB, SWAP_TAB, receiptHeadline(json)),
    false,
  );
});

test("the shell's swap-tab index still matches the tab order this test assumes", () => {
  // noticeDuplicatesReceipt is index-based; the shell resolves the index by
  // name through indexOfTab("swap"). Run the shipped resolver over the shipped
  // tab array to keep this file's constants honest.
  const tabsLiteral = shellQml.match(/readonly property var tabs:\s*(\[[\s\S]*?\])/);
  assert.ok(tabsLiteral, "AtomicSwapView.qml lost its tabs array");
  const root = { tabs: new Function(`return ${tabsLiteral[1]};`)() };
  const indexOfTab = new Function(
    "root",
    `${extractFunction(shellQml, "indexOfTab", "AtomicSwapView.qml")}; return indexOfTab;`,
  )(root);
  assert.equal(indexOfTab("swap"), SWAP_TAB);
  assert.equal(indexOfTab("setup"), SETUP_TAB);
  assert.equal(indexOfTab("no-such-tab"), 0);
});
