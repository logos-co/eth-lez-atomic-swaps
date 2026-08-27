#!/usr/bin/env node
// Unit + source-contract test for the guided Setup's step order and
// numbering (swap-ui/src/qml/SetupSteps.js) under BOTH flows of the hidden
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
//   * SetupView.qml renders every step header through the helpers rather
//     than a literal "N." string, and gates the faucet-less step on the real
//     initialized outcome, so a step can't go stale or hide the init step.
//   * the plugin's faucet-less step calls SwapImpl::lezEnsureInitialized
//     (its own path, not startLezFundingJob), and the funding job is still
//     the default flow's step — i.e. an account is initialized in BOTH
//     paths.
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

// --- Source contract: SetupView.qml uses the helpers, never literal "N." ---

const view = readFileSync(join(qmlDir, "SetupView.qml"), "utf8");
assert.ok(
  /import "SetupSteps\.js" as SetupSteps/.test(view),
  "SetupView.qml imports SetupSteps.js",
);
assert.ok(
  /subtitle:\s*SetupSteps\.subtitle\(/.test(view),
  "subtitle count comes from SetupSteps.subtitle",
);
const literalLabels = [...view.matchAll(/label:\s*"\d+\.\s/g)];
assert.strictEqual(
  literalLabels.length,
  0,
  `no hardcoded step numbers in SetupView.qml (found ${literalLabels.map((m) => m[0]).join(", ")})`,
);
// Every step id in either flow has exactly one rendered header.
for (const id of new Set([...DEFAULT, ...FAUCETLESS])) {
  const uses = [...view.matchAll(new RegExp(`SetupSteps\\.stepLabel\\("${id}",`, "g"))];
  assert.strictEqual(uses.length, 1, `SetupView.qml renders ${id} through stepLabel exactly once`);
}
// The two step-3 cards are mutually exclusive on the flag, so a mode never
// shows both (six cards) or neither (four cards).
assert.ok(
  /visible:\s*setupRoot\.faucetless[\s\S]*?stepLabel\("activateLez"/.test(view),
  "activate card is visible only in the faucet-less flow",
);
assert.ok(
  /visible:\s*!setupRoot\.faucetless[\s\S]*?stepLabel\("fundLez"/.test(view),
  "Fund LEZ card is visible only in the default flow",
);
// The faucet-less step is wired to the initialize-only slot and gates on the
// real outcome, never on a balance.
assert.ok(
  /onClicked:\s*swapBackend\.setupInitializeAccount\(\)/.test(view),
  "activate card calls setupInitializeAccount",
);
assert.ok(
  /lezReady:\s*setupRoot\.faucetless\s*\?\s*swapBackend\.setupInitialized/.test(view),
  "faucet-less readiness is gated on setupInitialized",
);
assert.ok(/isReady:\s*lezReady\s*&&\s*hasEthBalance/.test(view), "isReady still needs gas");
// Sellers keep a pinata path in the faucet-less flow, outside the numbered steps.
assert.ok(
  /id:\s*sellerFunding[\s\S]*?visible:\s*setupRoot\.faucetless[\s\S]*?setupStartFunding\(\)/.test(view),
  "seller funding under Advanced settings, faucet-less only, runs the funding job",
);

// --- Source contract: the plugin initializes in BOTH paths -----------------

const plugin = readFileSync(join(repo, "swap-ui", "src", "swap_ui_plugin.cpp"), "utf8");
const slot = plugin.slice(plugin.indexOf("void SwapUiPlugin::setupInitializeAccount()"));
assert.ok(slot.length > 0, "plugin implements setupInitializeAccount");
const slotBody = slot.slice(0, slot.indexOf("\n}\n") + 3);
assert.ok(
  /lezEnsureInitializedAsync\(/.test(slotBody),
  "faucet-less step calls lezEnsureInitialized (its own path)",
);
assert.ok(
  !/startLezFundingJobAsync\(/.test(slotBody),
  "faucet-less step never runs the pinata funding job",
);
assert.ok(
  /"Initialized"[\s\S]*?"AlreadyInitialized"[\s\S]*?setSetupInitialized\(true\)/.test(slotBody),
  "setupInitialized is set only on a recognised initialized outcome",
);
const funding = plugin.slice(plugin.indexOf("void SwapUiPlugin::setupStartFunding()"));
const fundingBody = funding.slice(0, funding.indexOf("\n}\n") + 3);
assert.ok(
  /startLezFundingJobAsync\(/.test(fundingBody),
  "default flow's step 3 still runs the (initialize-then-claim) funding job",
);
assert.ok(
  /setSetupTarget\(QStringLiteral\("150"\)\)/.test(fundingBody),
  "default flow's 150-LEZ target is untouched",
);
// The flag is read through the unit-tested parser, next to the other
// SWAP_UI_* overrides, and defaults to today's flow.
assert.ok(
  /qEnvironmentVariable\(swap_ui::kLezFaucetModeEnv\)/.test(plugin),
  "flag read via qEnvironmentVariable + setup_flow.h",
);
assert.ok(
  /setSetupFaucetless\(defaultSetupFaucetless\(\)\)/.test(plugin),
  "setupFaucetless resolved once at construction",
);
const header = readFileSync(join(repo, "swap-ui", "src", "setup_flow.h"), "utf8");
assert.ok(
  /kLezFaucetModeEnv = "SWAP_UI_LEZ_FAUCET_MODE"/.test(header),
  "flag name is SWAP_UI_LEZ_FAUCET_MODE",
);

// --- Source contract: replica + facade carry the new surface ---------------

const rep = readFileSync(join(repo, "swap-ui", "src", "swap_ui.rep"), "utf8");
for (const needle of [
  "PROP(bool setupFaucetless READWRITE)",
  "PROP(bool setupInitialized READWRITE)",
  "SLOT(void setupInitializeAccount())",
]) {
  assert.ok(rep.includes(needle), `swap_ui.rep declares ${needle}`);
}
const main = readFileSync(join(qmlDir, "Main.qml"), "utf8");
for (const needle of ["setupFaucetless", "setupInitialized", "setupInitializeAccount"]) {
  assert.ok(main.includes(needle), `Main.qml facade bridges ${needle}`);
}

console.log("ok   setup steps: default flow unchanged, faucet-less flow initializes, numbering honest in both");
