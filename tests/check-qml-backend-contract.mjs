#!/usr/bin/env node
// Source-contract check: every `swapBackend.<name>` a QML view references
// must exist on the swapBackend facade in Main.qml.
//
// Why: views run against the FACADE QtObject in Main.qml, not the replica
// directly. A backend property added to swap_ui.rep but not bridged in the
// facade silently reads as `undefined` in every view — and comparisons like
// `swapBackend.messagingHint !== ""` then evaluate TRUE forever, flipping
// views into permanent error/degraded states with no console error anywhere
// (exactly what happened with messagingPeerCountKnown/messagingHint in
// PR #113 review). This greppable contract turns that whole failure class
// into a test failure.
//
// Run: node tests/check-qml-backend-contract.mjs

import { readdirSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const qmlDir = join(dirname(fileURLToPath(import.meta.url)), "..", "swap-ui", "src", "qml");

const files = readdirSync(qmlDir).filter((f) => f.endsWith(".qml"));
if (!files.includes("Main.qml")) {
  console.error("FAIL: swap-ui/src/qml/Main.qml not found");
  process.exit(1);
}

// --- Facade surface: properties, functions, and signals declared on the
// swapBackend QtObject in Main.qml. Parsed textually (no QML engine in CI):
// the QtObject block starts at `id: swapBackend` and we take every
// declaration until the object's closing scope. To stay robust against
// re-indentation we simply scan the whole of Main.qml for declarations —
// Main.qml's only `property`/`function`/`signal` declarations outside the
// facade are on `root`, whose names (backend/backendReady/ready/watch) are
// harmless additions to the allowed set (`ready` is also a real facade
// property).
const mainSrc = readFileSync(join(qmlDir, "Main.qml"), "utf8");
const declared = new Set();
for (const m of mainSrc.matchAll(/^\s*(?:readonly\s+)?property\s+\w+\s+(\w+)/gm)) {
  declared.add(m[1]);
}
for (const m of mainSrc.matchAll(/^\s*function\s+(\w+)\s*\(/gm)) {
  declared.add(m[1]);
}
for (const m of mainSrc.matchAll(/^\s*signal\s+(\w+)/gm)) {
  declared.add(m[1]);
}

// Signal handlers views may attach (onXxxChanged for a property xxx, onXxx
// for a signal xxx) are derived from the declarations above.
const derived = new Set();
for (const name of declared) {
  derived.add("on" + name[0].toUpperCase() + name.slice(1)); // signals
  derived.add("on" + name[0].toUpperCase() + name.slice(1) + "Changed"); // properties
}

// --- References: every `swapBackend.<identifier>` in every other view file.
const failures = [];
let referenced = 0;
for (const file of files) {
  if (file === "Main.qml") continue;
  const src = readFileSync(join(qmlDir, file), "utf8");
  const lines = src.split("\n");
  lines.forEach((line, i) => {
    // Strip line comments so commented-out references don't count.
    const code = line.replace(/\/\/.*$/, "");
    for (const m of code.matchAll(/\bswapBackend\.(\w+)/g)) {
      referenced += 1;
      const name = m[1];
      if (!declared.has(name) && !derived.has(name)) {
        failures.push(`${file}:${i + 1}: swapBackend.${name} is not declared in the Main.qml facade`);
      }
    }
  });
}

if (failures.length > 0) {
  console.error("FAIL: QML views reference swapBackend members missing from the Main.qml facade:");
  for (const f of failures) {
    console.error("  " + f);
  }
  console.error(
    "\nBridge each one in Main.qml's swapBackend QtObject with a safe default\n" +
    "(an unbridged property reads as `undefined` in views — e.g. `x !== \"\"`\n" +
    "becomes permanently true)."
  );
  process.exit(1);
}

if (referenced === 0) {
  console.error("FAIL: no swapBackend.* references found — the checker is miswired");
  process.exit(1);
}

console.log(`qml backend facade contract: OK (${referenced} references across ${files.length - 1} views, ${declared.size} facade members)`);
