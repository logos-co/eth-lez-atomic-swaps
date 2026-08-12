// RFQ (request-for-quote) logic for the maker's offer-publisher sidecar.
//
// Pure, network-free helpers so they can be unit-tested without a Waku node
// (see rfq.test.mjs). The wiring that connects these to the fleet lives in
// publish-offer.mjs.
//
// The model: a taker publishes an ANONYMOUS offer-request on the requests
// topic when it opens the Market tab. A maker filter-subscribes to that topic
// and, on a valid request, publishes its current offer IMMEDIATELY — but
// COALESCED: at most one response per cooldown window regardless of how many
// requests arrive, so a burst of 100 pings produces exactly 1 offer. This is
// the key anti-amplification / anti-DoS mitigation: the requests topic is
// unauthenticated, so without coalescing an attacker could turn N cheap pings
// into N maker publishes (amplification). Malformed requests are ignored.
//
// The maker's slow fallback heartbeat (publish-offer.mjs) is unaffected and
// remains the reliable baseline: a taker who missed the RFQ response still
// gets an offer on the next heartbeat. RFQ is the accelerator, not the
// transport of record.

// Build an anonymous offer-request payload — the exact shape the C++ taker
// (swapDeliveryBuildOfferRequest) emits. Carries no identity/account: the
// privacy property the RFQ model rests on. Used by the RFQ smoke test to play
// the taker; the real taker builds this in C++.
export function buildOfferRequest(nowSecs = Math.floor(Date.now() / 1000)) {
  return { v: 1, kind: "offer_request", ts: nowSecs };
}

// Parse + validate an incoming request payload. Accepts a string or a byte
// array (Uint8Array / Buffer, as delivered by @waku filter subscriptions).
// Returns the parsed object when it is a well-formed offer-request, or null
// for anything malformed / unrecognized (which the caller silently ignores).
//
// Strict on the two fields that define the contract (kind + version) and
// tolerant of extra fields, so the schema can grow additively without makers
// needing to re-deploy in lockstep.
export function parseOfferRequest(payload) {
  let text;
  if (typeof payload === "string") {
    text = payload;
  } else if (payload && typeof payload.length === "number") {
    try {
      text = new TextDecoder("utf-8", { fatal: false }).decode(
        payload instanceof Uint8Array ? payload : Uint8Array.from(payload)
      );
    } catch {
      return null;
    }
  } else {
    return null;
  }

  let obj;
  try {
    obj = JSON.parse(text);
  } catch {
    return null;
  }
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    return null;
  }
  if (obj.kind !== "offer_request") {
    return null;
  }
  if (obj.v !== 1) {
    return null;
  }
  return obj;
}

// Coalescing responder gate. `tryAcquire(nowMs)` returns true at most once per
// `cooldownMs`: the first call in a cold window succeeds and arms the window;
// every call inside that window returns false. This is what turns any volume
// of requests into a single response per window.
//
// Time is injected (nowMs defaults to Date.now()) so tests drive it
// deterministically without sleeping.
export function createResponderGate(cooldownMs) {
  let lastResponseMs = Number.NEGATIVE_INFINITY;
  return {
    tryAcquire(nowMs = Date.now()) {
      if (nowMs - lastResponseMs < cooldownMs) {
        return false;
      }
      lastResponseMs = nowMs;
      return true;
    },
    // Milliseconds until the gate opens again (0 when already open). Handy for
    // logging / diagnostics.
    msUntilOpen(nowMs = Date.now()) {
      const remaining = cooldownMs - (nowMs - lastResponseMs);
      return remaining > 0 ? remaining : 0;
    },
    reset() {
      lastResponseMs = Number.NEGATIVE_INFINITY;
    },
  };
}
