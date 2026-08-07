import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const view = readFileSync("swap-ui/src/qml/AtomicSwapView.qml", "utf8");
const makerView = readFileSync("swap-ui/src/qml/MakerView.qml", "utf8");
const plugin = readFileSync("swap-ui/src/swap_ui_plugin.cpp", "utf8");

function between(source, start, end) {
  const startIndex = source.indexOf(start);
  const endIndex = source.indexOf(end, startIndex + start.length);
  assert.notEqual(startIndex, -1, `missing start marker: ${start}`);
  assert.notEqual(endIndex, -1, `missing end marker: ${end}`);
  return source.slice(startIndex, endIndex);
}

test("header Refresh remains available while a swap is running", () => {
  const button = between(view, "text: swapBackend.balancesLoading ? \"Refreshing\"", "onClicked: swapBackend.fetchBalances()");
  assert.match(button, /enabled:\s*!swapBackend\.balancesLoading/);
  assert.doesNotMatch(button, /!swapBackend\.running/);
});

test("maker completion requests one automatic balance refresh", () => {
  const completed = between(
    plugin,
    'step == QStringLiteral("AutoAcceptSwapCompleted")',
    'step == QStringLiteral("AutoAcceptSwapFailed")',
  );
  assert.equal((completed.match(/requestAutomaticBalanceRefresh\(\)/g) || []).length, 1);
});

test("automatic refresh uses only the loaded-env route", () => {
  const automatic = between(
    plugin,
    "void SwapUiPlugin::requestAutomaticBalanceRefresh()",
    "void SwapUiPlugin::completeBalanceRefresh",
  );
  assert.match(automatic, /!m_loadedEnvPath\.isEmpty\(\)/);
  assert.match(automatic, /fetchBalancesFromLoadedEnv\(\)/);
  assert.doesNotMatch(automatic, /fetchBalances\(\)/);
});

test("maker availability remains reactive to refreshed LEZ balance", () => {
  const availability = between(
    makerView,
    "var bal = swapBackend.lezBalance",
    "// --- Go Live Action ---",
  );
  assert.match(availability, /return "Available: "/);
});
