// Unit test for where a FAILED AUTOMATIC BALANCE READ is allowed to show up
// (issue #169).
//
// Every caller of fetchBalances() is automatic — the Market view's timers, the
// Setup tab's Sepolia-arrival poll, the post-key-generation refreshes and the
// post-swap settle poll. There is no user-initiated refresh. Publishing those
// failures into `errorMessage` therefore put a global red banner across a
// first-run app for something the user never triggered; routing them to the
// status line instead would have hidden a genuinely dead RPC. So they are
// published per side and shown where the user is already looking:
//
//   1. swap-ui/src/balance_error_policy.h decides WHETHER a side has failed
//      enough to say anything, and in what words. Covered by its own C++ unit
//      test (swap-ui/tests/balance_error_policy_test.cpp).
//   2. AtomicSwapView.qml's balanceNoticeText/balanceNoticeShows put it under
//      the account balances it describes, and stand it down on the Setup tab.
//   3. SetupView.qml's ethArrivalLine puts it inside step 4, ahead of copy
//      that would otherwise promise the app is watching a chain it cannot
//      reach.
//
// Layers 2 and 3 are tested here, evaluated straight out of the shipped QML
// (brace-matched, not hand-copied) so the shipped rule and the test cannot
// drift apart. Layer 1's wording is NOT pinned here — the QML helpers treat
// the sentence as an opaque string, so representative values are the right
// input. The one-place-per-failure rule of layer 2 is the same one
// tests/insufficient-eth-guard.test.mjs pins for the receipt strip.
//
// Run: node tests/balance-notice.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));

function source(...parts) {
  return readFileSync(join(here, "..", ...parts), "utf8");
}

const shellQml = source("swap-ui", "src", "qml", "AtomicSwapView.qml");
const setupQml = source("swap-ui", "src", "qml", "SetupView.qml");

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

function load(src, name, file) {
  return new Function(`${extractFunction(src, name, file)}; return ${name};`)();
}

const balanceNoticeText = load(shellQml, "balanceNoticeText", "AtomicSwapView.qml");
const balanceNoticeShows = load(shellQml, "balanceNoticeShows", "AtomicSwapView.qml");
const ethArrivalLine = load(setupQml, "ethArrivalLine", "SetupView.qml");

// Representative sentences. The authoritative wording lives in
// balanceErrorCopy() in swap-ui/src/balance_error_policy.h, and its behaviour
// (names the right chain, never carries raw RPC text, differs per side) is
// covered executably by swap-ui/tests/balance_error_policy_test.cpp. The QML
// helpers below treat whatever the policy publishes as an opaque string, so
// these only need to be plausible, not identical.
const ETH_DOWN =
  "Can't reach Ethereum right now, so the ETH balance may be out of date. "
  + "The app keeps trying.";
const LEZ_DOWN =
  "Can't reach the LEZ network right now, so the LEZ balance may be out of "
  + "date. The app keeps trying.";

const MARKET_TAB = 0;
const SETUP_TAB = 5;

test("a healthy app shows no balance notice at all", () => {
  assert.equal(balanceNoticeText("", ""), "");
  assert.equal(balanceNoticeShows("", MARKET_TAB, SETUP_TAB), false);
});

test("one dead chain shows only that chain's sentence", () => {
  assert.equal(balanceNoticeText(ETH_DOWN, ""), ETH_DOWN);
  assert.equal(balanceNoticeText("", LEZ_DOWN), LEZ_DOWN);
});

test("both dead reads as one notice, not two stacked alarms", () => {
  const both = balanceNoticeText(ETH_DOWN, LEZ_DOWN);
  assert.equal(both, `${ETH_DOWN} ${LEZ_DOWN}`);
  assert.equal(balanceNoticeShows(both, MARKET_TAB, SETUP_TAB), true);
});

test("the notice is visible on the Market surface — that is the point of it", () => {
  assert.equal(balanceNoticeShows(ETH_DOWN, MARKET_TAB, SETUP_TAB), true);
});

test("it stands down on Setup, which shows the same sentences in its steps", () => {
  // One failure, one place. Setup step 3 renders lezBalanceError and step 4
  // renders ethBalanceError; a strip repeating them above would read as two
  // separate problems.
  assert.equal(balanceNoticeShows(ETH_DOWN, SETUP_TAB, SETUP_TAB), false);
  assert.equal(
    balanceNoticeShows(`${ETH_DOWN} ${LEZ_DOWN}`, SETUP_TAB, SETUP_TAB),
    false,
  );
});

test("standing down never invents a notice out of nothing", () => {
  assert.equal(balanceNoticeShows("", SETUP_TAB, SETUP_TAB), false);
});

test("step 4 says the chain is unreachable instead of claiming to watch it", () => {
  // The bug this pins: "Watching for it… you don't need to do anything." while
  // the Sepolia RPC is down is untrue, and being unable to read the chain is
  // exactly why the test-ETH never appears.
  assert.equal(ethArrivalLine("", ETH_DOWN, 0, 40), ETH_DOWN);
  assert.equal(ethArrivalLine("", ETH_DOWN, 39, 40), ETH_DOWN);
});

test("step 4 keeps watching quietly while the chain is reachable", () => {
  assert.equal(
    ethArrivalLine("", "", 0, 40),
    "Watching for it… you don't need to do anything.",
  );
});

test("an exhausted poll budget still reads as 'still nothing', not a failure", () => {
  assert.equal(
    ethArrivalLine("", "", 40, 40),
    "Still nothing. Press Add funds' refresh, or reopen this tab to look again.",
  );
});

test("a balance that actually arrived outranks everything", () => {
  // A read that answered is proof the endpoint is alive, so a stale
  // unreachable message must never cover up the arrival the user is waiting
  // for.
  assert.equal(ethArrivalLine("Arrived — 0.05 ETH", ETH_DOWN, 40, 40), "Arrived — 0.05 ETH");
  assert.equal(ethArrivalLine("Arrived — 0.05 ETH", "", 0, 40), "Arrived — 0.05 ETH");
});
