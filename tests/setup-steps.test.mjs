#!/usr/bin/env node
// Unit test for the guided Setup's step order and numbering
// (swap-ui/src/qml/SetupSteps.js) under BOTH flows of the hidden
// SWAP_UI_LEZ_FAUCET_MODE flag (issue #166). The SAME file SetupView.qml
// imports is evaluated here, so this exercises the shipped logic, not a copy.
//
// Covers:
//   * default flow keeps today's steps: key, account, Fund LEZ, test ETH
//     (the "Start trading" completion card is unnumbered) — and the
//     faucet-less flow swaps Fund LEZ for an initialize-only "activate" step
//     (the pinata claim leaves the numbered flow).
//   * numbering is contiguous 1..N in both flows and the subtitle's count
//     word is derived from the same list (the "Four steps over five cards"
//     class of bug).
//
// The plugin side (setupInitializeAccount -> lezEnsureInitializedAsync, the
// setupInitialized gate) has no executable harness here; it is covered by
// the compiled plugin build and the fee-floor investigation, not by this test.
//
// Run: node tests/setup-steps.test.mjs

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import assert from "node:assert";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..");
const qmlDir = join(repo, "swap-ui", "src", "qml");

const src = readFileSync(join(qmlDir, "SetupSteps.js"), "utf8");
// Evaluate the real file and pull out its CommonJS exports.
const factory = new Function("module", src + "\n;return module.exports;");
const S = factory({ exports: {} });

// --- Step lists per mode -----------------------------------------------

const DEFAULT = S.stepsFor(false);
const FAUCETLESS = S.stepsFor(true);

assert.deepStrictEqual(
  DEFAULT,
  ["ethKey", "lezAccount", "fundLez", "testEth"],
  "default flow must keep today's four steps in today's order",
);
assert.deepStrictEqual(
  FAUCETLESS,
  ["ethKey", "lezAccount", "activateLez", "testEth"],
  "faucet-less flow: generate key -> initialize -> get Sepolia ETH -> trade",
);
assert.ok(!FAUCETLESS.includes("fundLez"), "pinata claim leaves the faucet-less flow");
assert.ok(FAUCETLESS.includes("activateLez"), "faucet-less flow must still initialize");
assert.ok(!DEFAULT.includes("activateLez"), "default flow is unchanged (no activate step)");

// --- Numbering is honest in both modes ------------------------------------

for (const [mode, steps] of [[false, DEFAULT], [true, FAUCETLESS]]) {
  const label = mode ? "faucet-less" : "default";
  steps.forEach((id, i) => {
    assert.strictEqual(S.stepNumber(id, mode), i + 1, `${label}: ${id} is step ${i + 1}`);
    assert.strictEqual(
      S.stepLabel(id, mode),
      `${i + 1}. ${S.TITLES[id]}`,
      `${label}: ${id} label carries its position`,
    );
    assert.ok(S.TITLES[id], `${label}: ${id} has a title`);
  });
  assert.strictEqual(S.stepCount(mode), steps.length);
  const sub = S.subtitle(mode);
  assert.ok(
    sub.startsWith(S.countWord(steps.length) + " steps"),
    `${label}: subtitle count is derived from the list (${sub})`,
  );
  assert.strictEqual(S.countWord(steps.length), "Four");
}
// Default flow: the rendered headers are byte-for-byte today's literals
// (what SetupView.qml carried before SetupSteps.js existed), so the flag
// being unset changes nothing the user reads on a card.
assert.deepStrictEqual(
  DEFAULT.map((id) => S.stepLabel(id, false)),
  ["1. Ethereum key", "2. LEZ account", "3. Fund LEZ", "4. Get test ETH"],
  "default-flow card headers are unchanged",
);
assert.strictEqual(S.subtitle(false), "Four steps, then you're trading. No keys to type.");
// The completion card is not a step in either flow: unnumbered, uncounted.
assert.strictEqual(S.stepNumber("trade", false), 0);
assert.strictEqual(S.stepNumber("trade", true), 0);
assert.strictEqual(S.TITLES.trade, "Start trading");
// A step outside the flow has no number and no label — its card is hidden.
assert.strictEqual(S.stepNumber("fundLez", true), 0);
assert.strictEqual(S.stepLabel("fundLez", true), "");
assert.strictEqual(S.stepNumber("activateLez", false), 0);
assert.strictEqual(S.stepLabel("activateLez", false), "");
assert.strictEqual(S.stepNumber("nope", false), 0);

console.log("ok   setup steps: default flow unchanged, faucet-less flow has its activate step, numbering honest in both");
