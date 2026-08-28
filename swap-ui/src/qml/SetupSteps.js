// Pure, engine-independent Setup step order + numbering.
//
// Shared by SetupView.qml (`import "SetupSteps.js" as SetupSteps`) and the
// node unit test (tests/setup-steps.test.mjs, which evaluates this exact
// file so it exercises the shipped code, not a copy). Deliberately plain
// JavaScript — no QML/Qt types, no DOM/Node globals — so both consumers
// behave identically.
//
// Why a module at all: the Setup page once carried a hardcoded "Four steps"
// subtitle over five numbered cards (fixed in #170 by taking the completion
// card out of the numbering), and the two onboarding flows of issue #166 swap
// a step in and out. Every number the page shows — the "N." on each card and
// the count in the subtitle — is derived here from ONE ordered list per mode,
// so neither can go stale against the other or against the cards actually
// rendered. The test pins that SetupView's cards use these helpers rather
// than literal numbers.

// Ordered ids of the NUMBERED steps in each mode. The faucet-less flow (the
// default) replaces the initialize-and-claim step with an initialize-only
// one; the pinata claim leaves the numbered flow entirely and lives in its
// own "Get test LEZ without trading" disclosure on the same page.
//
// "Start trading" is deliberately absent from both lists. It is the
// destination, not a step: it asks nothing of the user, and numbering it is
// exactly the bug #170 fixed. It keeps a TITLES entry, because SetupView
// still renders that card's header from here — it just never gets a number,
// so `stepNumber("trade", …)` is 0 and `stepLabel("trade", …)` is "" in
// every mode.
function stepsFor(faucetless) {
    return faucetless
        ? ["ethKey", "lezAccount", "activateLez", "testEth"]
        : ["ethKey", "lezAccount", "fundLez", "testEth"];
}

var TITLES = {
    ethKey: "Ethereum key",
    lezAccount: "LEZ account",
    fundLez: "Fund LEZ",
    activateLez: "Activate your LEZ account",
    testEth: "Get test ETH",
    // Not a numbered step — see stepsFor() above.
    trade: "Start trading"
};

// 1-based position of `id` in the mode's flow, or 0 when the step is not
// part of that flow (its card is hidden, or it is not a numbered step).
function stepNumber(id, faucetless) {
    var steps = stepsFor(faucetless);
    for (var i = 0; i < steps.length; i++) {
        if (steps[i] === id) return i + 1;
    }
    return 0;
}

// "3. Fund LEZ" — the card header for a step in this mode; "" when the
// step is not in the flow.
function stepLabel(id, faucetless) {
    var n = stepNumber(id, faucetless);
    if (n === 0) return "";
    return n + ". " + TITLES[id];
}

function stepCount(faucetless) {
    return stepsFor(faucetless).length;
}

var COUNT_WORDS = ["Zero", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine"];

function countWord(n) {
    return n >= 0 && n < COUNT_WORDS.length ? COUNT_WORDS[n] : String(n);
}

// The page subtitle, one whole sentence per step count.
//
// Spelled out rather than assembled from countWord() + a tail, because the
// release canary greps the SHIPPED files for the exact subtitle string
// (canary/release-content-expectations.json's qml_grep) and a sentence built
// at runtime is not there to find. Keyed BY the derived count, so it still
// cannot disagree with the cards: an unmapped count falls back to the
// assembled form rather than to a stale number.
var SUBTITLES = {
    4: "Four steps, then you're trading. No keys to type."
};

function subtitle(faucetless) {
    var n = stepCount(faucetless);
    return SUBTITLES[n] || (countWord(n) + " steps, then you're trading. No keys to type.");
}

if (typeof module !== "undefined" && module.exports) {
    module.exports = {
        stepsFor: stepsFor,
        TITLES: TITLES,
        stepNumber: stepNumber,
        stepLabel: stepLabel,
        stepCount: stepCount,
        countWord: countWord,
        SUBTITLES: SUBTITLES,
        subtitle: subtitle
    };
}
