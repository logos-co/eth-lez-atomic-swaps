// Unit test for weiToEth (swap-ui/src/qml/SwapFormat/Format.qml), the app's
// single ETH-amount formatter.
//
// Why this exists: the formatter used to switch units (ETH -> Gwei -> raw
// wei) as the amount shrank, so a small-but-real offer like 4500 Gwei
// (0.0000045 ETH) rendered as "4.5000 Gwei" instead of an ETH figure — the
// wrong unit surfaced on the Market board and the Swap "Buying" card
// (fix/eth-amounts-in-eth). The fix always returns ETH; this test pins that
// contract and the precision it depends on (dust amounts must not collapse
// to "0 ETH").
//
// The function is evaluated straight out of the shipped QML file (brace-
// matched, not hand-copied) so this exercises the real logic, not a parallel
// implementation of it.
//
// Run: node tests/amount-format.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(
  join(here, "..", "swap-ui", "src", "qml", "SwapFormat", "Format.qml"),
  "utf8",
);

function extractFunction(source, name) {
  const startRe = new RegExp(`function\\s+${name}\\s*\\([^)]*\\)\\s*\\{`);
  const startMatch = source.match(startRe);
  assert.ok(startMatch, `function ${name} not found in Format.qml`);
  let i = startMatch.index + startMatch[0].length;
  let depth = 1;
  while (depth > 0 && i < source.length) {
    if (source[i] === "{") depth += 1;
    else if (source[i] === "}") depth -= 1;
    i += 1;
  }
  return source.slice(startMatch.index, i);
}

const weiToEth = new Function(
  `${extractFunction(src, "weiToEth")}; return weiToEth;`,
)();

test("4500 Gwei worth of wei renders as a visible ETH amount, not 0", () => {
  // 4500 Gwei = 4500 * 1e9 wei = 0.0000045 ETH — the smallest offer on the
  // board (5 LEZ <-> 4500 Gwei). A naive 2-4 decimal format would show "0".
  assert.equal(weiToEth("4500000000000"), "0.0000045 ETH");
});

test("120000 Gwei worth of wei renders in ETH", () => {
  assert.equal(weiToEth("120000000000000"), "0.00012 ETH");
});

test("a round 1 ETH value has no trailing zeros or stray unit", () => {
  assert.equal(weiToEth("1000000000000000000"), "1 ETH");
});

test("zero and unparsable input render as 0 ETH, never blank or NaN", () => {
  assert.equal(weiToEth("0"), "0 ETH");
  assert.equal(weiToEth(""), "0 ETH");
  assert.equal(weiToEth(undefined), "0 ETH");
});

test("output is always ETH — the unit never switches to Gwei or raw wei", () => {
  for (const wei of ["1", "1000", "4500000000000", "120000000000000", "1000000000000000000"]) {
    const out = weiToEth(wei);
    assert.match(out, /ETH$/);
    assert.doesNotMatch(out, /Gwei/);
    assert.doesNotMatch(out, /wei$/);
  }
});
