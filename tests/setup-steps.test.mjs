#!/usr/bin/env node
// Unit + source-contract test for the guided Setup's step order and
// numbering (swap-ui/src/qml/SetupSteps.js) under BOTH onboarding flows of
// issue #166. The SAME file SetupView.qml imports is evaluated here, so this
// exercises the shipped logic, not a copy.
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
//   * SetupView.qml renders every step header through the helpers rather
//     than a literal "N." string, and gates the faucet-less step on the real
//     initialized outcome, so a step can't go stale or hide the init step.
//   * the faucet keeps an in-app home on the default path — its own named,
//     collapsed section on the Setup page, NOT an environment variable and
//     NOT buried in Advanced settings — and never appears twice in one flow.
//   * the plugin's faucet-less step calls SwapImpl::lezEnsureInitialized
//     (its own path, not startLezFundingJob), and the funding job is still
//     the legacy flow's step — i.e. an account is initialized in BOTH paths.
//   * issue #171: that call BLOCKS for up to 300s, so it is issued with an
//     explicit Timeout that outlasts the commit wait, every answer is judged
//     by swap_ui::classifyActivation (which re-checks before declaring
//     failure and never reports a blank detail), and the wait has a visible
//     phase the user can read. The decision itself is unit-tested in
//     swap-ui/tests/lez_activation_test.cpp; this pins the wiring.
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
// canary/release-content-expectations.json greps the shipped files for this
// exact string; keep it findable as one literal.
const expectations = JSON.parse(
  readFileSync(join(repo, "canary", "release-content-expectations.json"), "utf8"),
);
const shipped = readFileSync(join(qmlDir, "SetupSteps.js"), "utf8");
for (const [version, entry] of Object.entries(expectations.modules.swap_ui)) {
  for (const marker of entry.qml_grep || []) {
    if (!marker.includes("steps, then you're trading") && marker !== "Start trading") continue;
    assert.ok(
      shipped.includes(marker) || view.includes(marker),
      `swap_ui@${version} canary marker is still a literal in a shipped file: ${marker}`,
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
// Every numbered step id in either flow has exactly one rendered header.
for (const id of new Set([...DEFAULT_FLOW, ...LEGACY])) {
  const uses = [...view.matchAll(new RegExp(`SetupSteps\\.stepLabel\\("${id}",`, "g"))];
  assert.strictEqual(uses.length, 1, `SetupView.qml renders ${id} through stepLabel exactly once`);
}
// #170, from the other side: the completion card must NOT go back through
// stepLabel, or it starts wearing a number again.
assert.ok(
  !/SetupSteps\.stepLabel\("trade"/.test(view),
  'the "Start trading" card is not labelled through stepLabel',
);
// It renders its header from SetupSteps.TITLES, so the one place that owns
// step wording owns this card's wording too — it simply never gets a number.
assert.ok(
  /label:\s*SetupSteps\.TITLES\.trade/.test(view),
  "the completion card's header comes from SetupSteps.TITLES.trade",
);
assert.strictEqual(S.TITLES.trade, "Start trading");
// The two step-3 cards are mutually exclusive on the flag, so a mode never
// shows both (five cards) or neither (three cards).
assert.ok(
  /visible:\s*setupRoot\.faucetless[\s\S]*?stepLabel\("activateLez"/.test(view),
  "activate card is visible only in the faucet-less (default) flow",
);
assert.ok(
  /visible:\s*!setupRoot\.faucetless[\s\S]*?stepLabel\("fundLez"/.test(view),
  "Fund LEZ card is visible only in the legacy flow",
);
// The faucet-less step is wired to the initialize-only slot and gates on the
// real outcome, never on a balance.
assert.ok(
  /onClicked:\s*\{[^}]*swapBackend\.setupInitializeAccount\(\)/.test(view),
  "activate card calls setupInitializeAccount",
);
assert.ok(
  /lezReady:\s*setupRoot\.faucetless\s*\?\s*swapBackend\.setupInitialized/.test(view),
  "faucet-less readiness is gated on setupInitialized",
);
assert.ok(/isReady:\s*lezReady\s*&&\s*hasEthBalance/.test(view), "isReady still needs gas");

// --- Source contract: the faucet has an in-app home on the default path ----

const faucetSection = view.slice(view.indexOf("id: otherWaysToGetLez"));
assert.ok(faucetSection.length > 0, "SetupView.qml has an otherWaysToGetLez section");
const faucetBody = faucetSection.slice(0, faucetSection.indexOf("\n    // --- Advanced"));
assert.ok(faucetBody.length > 0, "the faucet section sits above Advanced settings");
assert.ok(
  /Disclosure\s*\{\s*\n\s*id:\s*otherWaysToGetLez/.test(view),
  "the faucet lives in a Disclosure — collapsed by default, so the primary path stays short",
);
assert.ok(
  /visible:\s*setupRoot\.faucetless/.test(faucetBody),
  "the faucet section shows only on the faucet-less path (the legacy flow has it as step 3)",
);
assert.ok(
  /label:\s*"Get test LEZ without trading"/.test(faucetBody),
  "the section is named for what the user gets, not for the word 'faucet'",
);
assert.ok(
  /swapBackend\.setupStartFunding\(\)/.test(faucetBody),
  "the section actually runs the pinata funding job",
);
// One claim affordance per flow. setupStartFunding is called from the legacy
// step-3 card and from this section, and those two are mutually exclusive.
assert.strictEqual(
  [...view.matchAll(/swapBackend\.setupStartFunding\(\)/g)].length,
  2,
  "exactly two callers of setupStartFunding: legacy step 3 and the faucet section",
);
// It is no longer buried inside the raw-config section — "Advanced settings"
// is where fields live, not where you go to get coins.
const advancedBody = view.slice(view.indexOf('label: "Advanced settings"'));
assert.ok(
  !/setupStartFunding\(\)/.test(advancedBody),
  "the claim is not hidden inside Advanced settings",
);
// No dead ends: a claim started here reports progress, failure and outcome
// where it was started, rather than inside the numbered step that shares the
// backend's setupRunning/setupStep/setupError PROPs.
assert.ok(
  /property string setupOrigin/.test(view),
  "the page tracks which affordance owns the shared setup* PROPs",
);
for (const needle of [
  /setupRoot\.setupOrigin = "claim"/,
  /setupRoot\.setupOrigin = "activate"/,
]) {
  assert.ok(needle.test(view), `setupOrigin is set on click (${needle})`);
}
assert.ok(
  /visible:\s*setupRoot\.setupOrigin === "claim"\s*&&\s*swapBackend\.setupError !== ""/.test(faucetBody),
  "claim errors surface in the faucet section",
);
assert.ok(
  /visible:\s*setupRoot\.setupOrigin !== "claim"\s*&&\s*swapBackend\.setupError !== ""/.test(view),
  "the activate step hides errors that belong to a claim",
);

// --- Source contract: the plugin initializes in BOTH paths -----------------

const plugin = readFileSync(join(repo, "swap-ui", "src", "swap_ui_plugin.cpp"), "utf8");
const slot = plugin.slice(plugin.indexOf("void SwapUiPlugin::setupInitializeAccount()"));
assert.ok(slot.length > 0, "plugin implements setupInitializeAccount");
const slotBody = slot.slice(0, slot.indexOf("\n}\n") + 3);
assert.ok(
  /runLezActivation\(false\)/.test(slotBody),
  "faucet-less step runs the activation path (lezEnsureInitialized; see #171 below)",
);
assert.ok(
  !/startLezFundingJobAsync\(/.test(slotBody),
  "faucet-less step never runs the pinata funding job",
);
// The strict outcome check that must not be relaxed lives in the unit-tested
// header now (swap-ui/tests/lez_activation_test.cpp pins it exhaustively).
const activationHeaderSrc = readFileSync(
  join(repo, "swap-ui", "src", "lez_activation.h"),
  "utf8",
);
assert.ok(
  /kOutcomeInitialized = "Initialized"/.test(activationHeaderSrc)
    && /kOutcomeAlreadyInitialized = "AlreadyInitialized"/.test(activationHeaderSrc),
  "only Initialized/AlreadyInitialized count as an activated account",
);
const funding = plugin.slice(plugin.indexOf("void SwapUiPlugin::setupStartFunding()"));
const fundingBody = funding.slice(0, funding.indexOf("\n}\n") + 3);
assert.ok(
  /startLezFundingJobAsync\(/.test(fundingBody),
  "the pinata claim still runs the (initialize-then-claim) funding job",
);
assert.ok(
  /setSetupTarget\(QStringLiteral\("150"\)\)/.test(fundingBody),
  "the 150-LEZ claim target is untouched",
);
// The flag is read through the unit-tested parser, next to the other
// SWAP_UI_* overrides.
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
// The default lives in one expression, and it is the faucet-less one. The
// C++ side pins the parse itself (swap-ui/tests/setup_flow_test.cpp).
assert.ok(
  /return value == "on" \? LezFaucetMode::On : LezFaucetMode::Off;/.test(header),
  "only an exact \"on\" selects the legacy flow; everything else is the default",
);

// --- Source contract: issue #171, the activation call that blocks ---------
//
// swap_ffi_lez_ensure_initialized waits out the Initialize commit (up to
// INIT_COMMIT_TIMEOUT = 300s) while the generated Async wrapper's DEFAULT
// Timeout is 20s and turns its expiry into an empty QString. That is how the
// captain got "activation failed" for an activation that had succeeded.

const activationHeader = activationHeaderSrc;
assert.ok(
  /kActivationTimeoutMs = 330 \* 1000/.test(activationHeader),
  "the activation transport budget outlasts the 300s commit wait",
);
const activate = plugin.slice(plugin.indexOf("void SwapUiPlugin::runLezActivation(bool rechecked)"));
assert.ok(activate.length > 0, "plugin has a single-attempt runLezActivation");
const activateBody = activate.slice(0, activate.indexOf("\n}\n") + 3);
assert.ok(
  /lezEnsureInitializedAsync\([\s\S]*?Timeout\(swap_ui::kActivationTimeoutMs\)/.test(activateBody),
  "the blocking call is issued with the explicit long Timeout, not the 20s default",
);
// The strict outcome check stays, but it lives in the tested header now — and
// the plugin must not have kept its own copy of the old fall-through.
assert.ok(
  /swap_ui::classifyActivation\(/.test(plugin),
  "activation answers are judged by the unit-tested classifier",
);
assert.ok(
  !/QStringLiteral\("Unexpected activation result/.test(plugin),
  "the plugin no longer builds that message itself (it can be blank; lez_activation.h guards it)",
);
assert.ok(
  /ActivationVerdict::Retry[\s\S]*?runLezActivation\(true\)/.test(plugin),
  "an inconclusive answer re-checks instead of failing",
);
assert.ok(
  /ActivationVerdict::Succeeded[\s\S]*?setSetupInitialized\(true\)/.test(plugin),
  "only a Succeeded verdict marks the account initialized",
);
// The waiting state is visible, and both phase names have UI copy — a step
// the QML cannot name would render as the raw enum string.
for (const step of ["AwaitingCommit", "Verifying"]) {
  assert.ok(
    new RegExp(`"${step}":\\s*"`).test(view),
    `SetupView.humanSetupStep has copy for the ${step} phase`,
  );
  assert.ok(
    new RegExp(`kStep${step === "AwaitingCommit" ? "AwaitingCommit" : "Verifying"}`).test(activationHeader),
    `${step} is named once, in lez_activation.h`,
  );
}
assert.ok(
  /kStepAwaitingCommit/.test(plugin) && /kStepVerifying/.test(plugin),
  "the plugin sets both visible phases from the shared constants",
);

console.log(
  "ok   setup steps: faucet-less default, faucet reachable in-app, four honest steps in both flows",
);
console.log("ok   activation (#171): long timeout, re-check before failure, visible waiting state");
