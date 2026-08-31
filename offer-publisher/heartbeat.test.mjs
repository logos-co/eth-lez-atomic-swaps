// Pure unit tests for the one heartbeat knob. Run: node offer-publisher/heartbeat.test.mjs
//
// These pin the contract that made the old two-variable setup confusing:
// OFFER_HEARTBEAT_SECS is the single source of truth, FALLBACK_HEARTBEAT_SECS
// is a deprecated alias that still works but always warns, and the default is
// 30 in every path (and matches src/cli/maker.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  resolveHeartbeatSecs,
  DEFAULT_HEARTBEAT_SECS,
  CANONICAL_ENV,
  DEPRECATED_ENV,
} from "./heartbeat.mjs";

test("default is 30s and warns about nothing when neither var is set", () => {
  const r = resolveHeartbeatSecs({});
  assert.equal(r.secs, 30);
  assert.equal(DEFAULT_HEARTBEAT_SECS, 30);
  assert.equal(r.source, "default");
  assert.deepEqual(r.warnings, []);
});

test("OFFER_HEARTBEAT_SECS alone is used silently", () => {
  const r = resolveHeartbeatSecs({ [CANONICAL_ENV]: "45" });
  assert.equal(r.secs, 45);
  assert.equal(r.source, CANONICAL_ENV);
  assert.deepEqual(r.warnings, []);
});

test("the deprecated alias still works, but warns", () => {
  const r = resolveHeartbeatSecs({ [DEPRECATED_ENV]: "20" });
  assert.equal(r.secs, 20, "alias must keep working — deployments already set it");
  assert.equal(r.source, DEPRECATED_ENV);
  assert.equal(r.warnings.length, 1);
  assert.match(r.warnings[0], /DEPRECATED/);
  assert.match(r.warnings[0], new RegExp(CANONICAL_ENV));
});

test("canonical wins over the alias, and says so", () => {
  // The exact confusion this replaces: the sidecar used to prefer the alias,
  // so an OFFER_HEARTBEAT_SECS injected by swap-cli was silently overridden.
  const r = resolveHeartbeatSecs({ [CANONICAL_ENV]: "30", [DEPRECATED_ENV]: "45" });
  assert.equal(r.secs, 30);
  assert.equal(r.source, CANONICAL_ENV);
  assert.equal(r.warnings.length, 1);
  assert.match(r.warnings[0], /IGNORED/);
});

test("unusable values never become a cadence", () => {
  for (const bad of ["0", "-5", "abc", "1.5", "  "]) {
    const r = resolveHeartbeatSecs({ [CANONICAL_ENV]: bad });
    assert.equal(r.secs, DEFAULT_HEARTBEAT_SECS, `${bad} must not become an interval`);
    assert.equal(r.source, "default");
  }
  // A blank var is simply unset — not worth a warning.
  assert.deepEqual(resolveHeartbeatSecs({ [CANONICAL_ENV]: "" }).warnings, []);
  // A set-but-garbage one is.
  assert.equal(resolveHeartbeatSecs({ [CANONICAL_ENV]: "abc" }).warnings.length, 1);
});

test("a garbage canonical value falls through to a usable alias", () => {
  const r = resolveHeartbeatSecs({ [CANONICAL_ENV]: "abc", [DEPRECATED_ENV]: "25" });
  assert.equal(r.secs, 25);
  assert.equal(r.source, DEPRECATED_ENV);
  assert.equal(r.warnings.length, 2, "both the bad value and the deprecation are reported");
});
