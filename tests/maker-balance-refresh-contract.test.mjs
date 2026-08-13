// Contract: the account strip's balances keep themselves correct.
//
// Rewritten for the redesign. The old version encoded the opposite contract —
// that a manual "Refresh" button stays enabled while a swap runs — because at
// the time that button was the only thing that actually worked. The button is
// gone, so the automatic path has to carry it, and these tests pin the pieces
// that make that true.
//
// Background (issue #57): the C++ automatic path is env-only. Every completion
// hook calls beginBalanceSettle() -> requestAutomaticBalanceRefresh(), which
// calls fetchBalancesFromLoadedEnv() gated on `!m_loadedEnvPath.isEmpty()`.
// m_loadedEnvPath is set in exactly one place — a successful loadEnvFile() — so
// for anyone who configured through Setup it is empty and every automatic
// refresh silently no-ops. The QML side drives fetchBalances() (the
// config-backed route) instead of changing that C++ contract.
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

test("the account strip has no manual Refresh button", () => {
  // If someone reintroduces one, the automatic path below stops being
  // load-bearing and will rot again without anyone noticing.
  assert.doesNotMatch(view, /text:\s*"Refresh"/);
  assert.doesNotMatch(view, /"Refreshing"/);
});

test("balances refresh automatically on every event that moves them", () => {
  const handlers = between(view, "property int balanceSettleTicks", "ColumnLayout {");

  // The sale-completed case #57 actually reported.
  assert.match(handlers, /function onAutoAcceptCompletedChanged\(\)/);
  // Both directions of a finished run, plus refunds.
  assert.match(handlers, /function onTakerRunningChanged\(\)/);
  assert.match(handlers, /function onMakerRunningChanged\(\)/);
  assert.match(handlers, /function onRefundsLoadingChanged\(\)/);

  // Must use the config-backed route; fetchBalancesFromLoadedEnv is not
  // reachable from QML and would be the broken path anyway.
  assert.match(handlers, /swapBackend\.fetchBalances\(\)/);
});

test("a completed swap keeps re-reading, not just once", () => {
  // One read the instant a job reports done can land before the chain write is
  // visible. refreshBalancesSoon arms a repeating settle window.
  const settle = between(view, "function refreshBalancesSoon", "// First read");
  assert.match(settle, /balanceSettleTicks\s*=\s*[1-9]/);
  assert.match(settle, /repeat:\s*true/);
});

test("automatic refresh in C++ is still env-only (why the QML fix exists)", () => {
  const automatic = between(
    plugin,
    "void SwapUiPlugin::requestAutomaticBalanceRefresh()",
    "void SwapUiPlugin::completeBalanceRefresh",
  );
  assert.match(automatic, /!m_loadedEnvPath\.isEmpty\(\)/);
  assert.match(automatic, /fetchBalancesFromLoadedEnv\(\)/);
  assert.doesNotMatch(automatic, /[^m]fetchBalances\(\)/);
});

test("maker availability remains reactive to refreshed LEZ balance", () => {
  // The whole point of the automatic refresh above: this string has to be
  // derived from the live balance, not captured once.
  const availability = between(
    makerView,
    "var bal = swapBackend.lezBalance",
    "// --- Go live ---",
  );
  assert.match(availability, /return "Available: "/);
});
