// Pure, engine-independent Setup step order + numbering.
//
// Shared by SetupView.qml (`import "SetupSteps.js" as SetupSteps`) and the
// node unit test (tests/setup-steps.test.mjs, which evaluates this exact
// file so it exercises the shipped code, not a copy). Deliberately plain
// JavaScript — no QML/Qt types, no DOM/Node globals — so both consumers
// behave identically.
//
// Why a module at all: the Setup page once carried a hardcoded "Four steps"
// subtitle over five numbered cards, and the hidden faucet-less flag (issue
// #166, SWAP_UI_LEZ_FAUCET_MODE=off) swaps a step in and out. Every number
// the page shows — the "N." on each card and the count in the subtitle — is
// derived here from ONE ordered list per mode, so neither can go stale
// against the other or against the cards actually rendered. The test pins
// that SetupView's cards use these helpers rather than literal numbers.

// Ordered step ids for each mode. The faucet-less flow replaces the
// initialize-and-claim step with an initialize-only one; the pinata claim
// leaves the numbered flow and lives under Advanced settings for sellers.
// The "Start trading" completion card is deliberately NOT a step: it is
// where you land once these are done, so it is unnumbered and not counted.
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
    trade: "Start trading"
};

// 1-based position of `id` in the mode's flow, or 0 when the step is not
// part of that flow (its card is hidden).
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

// The page subtitle, with its count derived from the same list as the
// card numbers.
function subtitle(faucetless) {
    return countWord(stepCount(faucetless)) + " steps, then you're trading. No keys to type.";
}

if (typeof module !== "undefined" && module.exports) {
    module.exports = {
        stepsFor: stepsFor,
        TITLES: TITLES,
        stepNumber: stepNumber,
        stepLabel: stepLabel,
        stepCount: stepCount,
        countWord: countWord,
        subtitle: subtitle
    };
}
