// Pure unit tests for the RFQ logic (no Waku node). Run: node --test
//
// Covers the four behaviours the RFQ model rests on: request payload
// build/parse, malformed-request rejection, responder coalescing (N requests
// in a window -> 1 response), and that the fallback path is independent of the
// responder (the gate never blocks forever).

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildOfferRequest,
  parseOfferRequest,
  createResponderGate,
} from "./rfq.mjs";

test("buildOfferRequest is anonymous and round-trips through parse", () => {
  const req = buildOfferRequest(1700000000);
  assert.deepEqual(req, { v: 1, kind: "offer_request", ts: 1700000000 });
  // No identity/account fields — the privacy invariant.
  assert.deepEqual(Object.keys(req).sort(), ["kind", "ts", "v"]);
  const parsed = parseOfferRequest(JSON.stringify(req));
  assert.ok(parsed);
  assert.equal(parsed.kind, "offer_request");
});

test("parseOfferRequest accepts strings and byte arrays", () => {
  const text = JSON.stringify(buildOfferRequest(42));
  assert.ok(parseOfferRequest(text));
  assert.ok(parseOfferRequest(new TextEncoder().encode(text)));
  assert.ok(parseOfferRequest(Buffer.from(text)));
});

test("parseOfferRequest ignores malformed / unrecognized requests", () => {
  const bad = [
    "",
    "not json",
    "{",
    "[]",
    "null",
    "42",
    JSON.stringify({ v: 1 }), // no kind
    JSON.stringify({ kind: "offer_request" }), // no version
    JSON.stringify({ v: 2, kind: "offer_request" }), // wrong version
    JSON.stringify({ v: 1, kind: "offer" }), // wrong kind
    JSON.stringify(["offer_request"]),
    undefined,
    null,
    {},
  ];
  for (const b of bad) {
    assert.equal(parseOfferRequest(b), null, `should reject: ${String(b)}`);
  }
  // Extra fields are tolerated (additive schema growth).
  assert.ok(parseOfferRequest(JSON.stringify({ v: 1, kind: "offer_request", ts: 1, extra: "x" })));
});

test("responder gate coalesces a burst into one response per window", () => {
  const cooldownMs = 8000;
  const gate = createResponderGate(cooldownMs);

  // A burst of 100 requests at the same instant -> exactly one response.
  let responses = 0;
  const t0 = 1_000_000;
  for (let i = 0; i < 100; i++) {
    if (gate.tryAcquire(t0)) responses++;
  }
  assert.equal(responses, 1, "100 simultaneous pings must yield 1 response");

  // Still closed just before the window elapses...
  assert.equal(gate.tryAcquire(t0 + cooldownMs - 1), false);
  // ...and open again exactly at the window boundary.
  assert.equal(gate.tryAcquire(t0 + cooldownMs), true);
});

test("responder gate opens for the first ever request (cold start)", () => {
  const gate = createResponderGate(8000);
  assert.equal(gate.tryAcquire(0), true);
});

test("msUntilOpen reports the remaining cooldown", () => {
  const gate = createResponderGate(8000);
  gate.tryAcquire(1000);
  assert.equal(gate.msUntilOpen(1000), 8000);
  assert.equal(gate.msUntilOpen(5000), 4000);
  assert.equal(gate.msUntilOpen(9000), 0);
  assert.equal(gate.msUntilOpen(20000), 0);
});
