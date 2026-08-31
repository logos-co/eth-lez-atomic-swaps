// Contract: the account strip's balances keep themselves correct.
//
// Rewritten for the redesign. The old version encoded the opposite contract —
// that a manual "Refresh" button stays enabled while a swap runs — because at
// the time that button was the only thing that actually worked. The button is
// gone, so the automatic path has to carry it, and these tests pin the pieces
// that make that true.
//
// Background (issue #57): the C++ automatic path used to be env-only. Every
// completion hook calls beginBalanceSettle() -> requestAutomaticBalanceRefresh(),
// which called fetchBalancesFromLoadedEnv() gated on `!m_loadedEnvPath.isEmpty()`.
// m_loadedEnvPath is set in exactly one place — a successful loadEnvFile() — so
// for anyone who configured through Setup it is empty and every automatic
// refresh silently no-opped. Two things carry it now: the QML ticks below for
// the fast case, and the C++ path itself, which routes to the config-backed
// fetchBalances() when there is no loaded env file. The C++ half is what covers
// a leg that confirms a block after the swap reports finished — the taker's LEZ
// claim, which is submitted, not committed, when run_taker returns.
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

test("automatic refresh in C++ reaches config-backed users too", () => {
  // The inverse of what this file used to assert. While the automatic path
  // knew only the env-file route, the post-swap settle poll reached nobody who
  // configured through Setup, and a completed swap left the leg that lands
  // late — the taker's received LEZ — stale until the app was restarted.
  const automatic = between(
    plugin,
    "void SwapUiPlugin::requestAutomaticBalanceRefresh()",
    "BalanceSnapshot SwapUiPlugin::balanceSnapshot",
  );
  assert.match(automatic, /Decision::StartFromEnv/);
  assert.match(automatic, /fetchBalancesFromLoadedEnv\(\)/);
  assert.match(automatic, /Decision::StartFromConfig/);
  assert.match(automatic, /[^m]fetchBalances\(\)/);
});

test("the post-swap settle poll waits for BOTH legs, not the first one", () => {
  // Behaviour is covered in swap-ui/tests/balance_refresh_coordinator_test.cpp;
  // this pins that the plugin actually hands the coordinator both sides and a
  // window long enough for a LEZ block, rather than a joined key that any one
  // side moving would satisfy.
  const settle = between(
    plugin,
    "void SwapUiPlugin::beginBalanceSettle()",
    "void SwapUiPlugin::completeBalanceRefresh",
  );
  assert.match(settle, /beginSettle\(balanceSnapshot\(\)/);
  assert.match(settle, /kBalanceSettleWindowMs/);

  const header = readFileSync("swap-ui/src/swap_ui_plugin.h", "utf8");
  const windowMs = Number(
    header.match(/kBalanceSettleWindowMs\s*=\s*(\d+)/)?.[1] ?? 0,
  );
  // A LEZ block on the public testnet can be a minute or more apart, and the
  // claim is only submitted when the swap reports finished.
  assert.ok(
    windowMs >= 120000,
    `settle window ${windowMs}ms is too short for a LEZ claim to commit`,
  );
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
