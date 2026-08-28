// Unit test for the guided Setup's step order and numbering
// (swap-ui/src/qml/SetupSteps.js) under BOTH onboarding flows of issue #166.
// The SAME file SetupView.qml imports is evaluated here, so this exercises
// the shipped logic, not a copy.
//
// Covers:
//   * the DEFAULT flow is faucet-less — key, LEZ account, activate, test ETH
//     — and SWAP_UI_LEZ_FAUCET_MODE=on's legacy flow swaps the activate step
//     back for "Fund LEZ". The pinata claim is not a numbered step in the
//     default flow.
//   * numbering is contiguous 1..N in both flows, the subtitle's count word
//     is derived from the same list (the "Four steps over five cards" class
//     of bug), and the completion card stays OUT of the numbering in every
//     mode (the #170 fix, which must not regress).
//
// The flag parsing and the #171 activation decision are covered executably
// by swap-ui/tests/setup_flow_test.cpp and swap-ui/tests/lez_activation_test.cpp.
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
//
// `faucetless === true` is the DEFAULT the app ships (setup_flow.h resolves
// anything but an exact "on" to it); `false` is the legacy developer flow.

const LEGACY = S.stepsFor(false);
const DEFAULT_FLOW = S.stepsFor(true);

assert.deepStrictEqual(
  DEFAULT_FLOW,
  ["ethKey", "lezAccount", "activateLez", "testEth"],
  "default flow: generate key -> activate account -> get Sepolia ETH",
);
assert.deepStrictEqual(
  LEGACY,
  ["ethKey", "lezAccount", "fundLez", "testEth"],
  "legacy (SWAP_UI_LEZ_FAUCET_MODE=on) flow keeps the Fund LEZ step",
);
assert.ok(!DEFAULT_FLOW.includes("fundLez"), "the pinata claim is not a step on the default path");
assert.ok(DEFAULT_FLOW.includes("activateLez"), "default flow must still initialize the account");
assert.ok(!LEGACY.includes("activateLez"), "legacy flow is unchanged (no activate step)");

// --- Numbering is honest in both modes ------------------------------------

for (const [mode, steps] of [[true, DEFAULT_FLOW], [false, LEGACY]]) {
  const label = mode ? "default" : "legacy";
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
  // Both flows are four cards deep, so the promise the page opens with is
  // the same sentence either way — and it is the sentence #170 made true.
  assert.strictEqual(S.countWord(steps.length), "Four", `${label}: four numbered steps`);
  assert.strictEqual(
    sub,
    "Four steps, then you're trading. No keys to type.",
    `${label}: subtitle matches the cards`,
  );
  // #170: the completion card is the destination, not a step. Numbering it
  // is exactly the bug that made "Four steps" sit over five cards.
  assert.strictEqual(S.stepNumber("trade", mode), 0, `${label}: "Start trading" is unnumbered`);
  assert.strictEqual(S.stepLabel("trade", mode), "", `${label}: "Start trading" has no step label`);
  assert.ok(!steps.includes("trade"), `${label}: "trade" is not in the numbered list`);
}

// Every spelled-out subtitle agrees with the count it is keyed by, so the
// contiguous-sentence form the release canary greps for can never drift from
// the derived number.
for (const [count, sentence] of Object.entries(S.SUBTITLES)) {
  assert.ok(
    sentence.startsWith(S.countWord(Number(count)) + " steps,"),
    `SUBTITLES[${count}] opens with "${S.countWord(Number(count))} steps," (${sentence})`,
  );
}
// Text contract, not a behaviour proxy: canary/release-content-expectations.json's
// qml_grep legs assert that exact user-facing literals are present in the
// SHIPPED files, so the subtitle must exist as one contiguous literal in
// SetupSteps.js rather than be assembled at runtime. subtitle()'s behaviour
// itself is asserted above; this only pins the literal the canary greps for.
const expectations = JSON.parse(
  readFileSync(join(repo, "canary", "release-content-expectations.json"), "utf8"),
);
for (const [version, entry] of Object.entries(expectations.modules.swap_ui)) {
  for (const marker of entry.qml_grep || []) {
    if (!marker.includes("steps, then you're trading")) continue;
    assert.ok(
      src.includes(marker),
      `swap_ui@${version} canary subtitle marker is a contiguous literal in SetupSteps.js: ${marker}`,
    );
  }
}

// The rendered headers, spelled out, so a change to either flow's wording or
// order has to be made deliberately here too.
assert.deepStrictEqual(
  DEFAULT_FLOW.map((id) => S.stepLabel(id, true)),
  ["1. Ethereum key", "2. LEZ account", "3. Activate your LEZ account", "4. Get test ETH"],
);
assert.deepStrictEqual(
  LEGACY.map((id) => S.stepLabel(id, false)),
  ["1. Ethereum key", "2. LEZ account", "3. Fund LEZ", "4. Get test ETH"],
  "legacy-flow card headers are byte-for-byte what 0.4.5 shipped",
);
// A step outside the flow has no number and no label — its card is hidden.
assert.strictEqual(S.stepNumber("fundLez", true), 0);
assert.strictEqual(S.stepLabel("fundLez", true), "");
assert.strictEqual(S.stepNumber("activateLez", false), 0);
assert.strictEqual(S.stepLabel("activateLez", false), "");
assert.strictEqual(S.stepNumber("nope", false), 0);
assert.strictEqual(S.TITLES.trade, "Start trading");

console.log("setup-steps: ok");
