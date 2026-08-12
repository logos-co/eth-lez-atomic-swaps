// Pure, engine-independent offer-board safety filters.
//
// Shared by OfferBoard.qml (`import "OfferFilter.js" as OfferFilter`) and the
// node unit test (tests/offer-filter.test.mjs, which evaluates this exact file
// so it exercises the shipped code, not a copy). Deliberately plain JavaScript
// — no QML/Qt types, no DOM/Node globals — so both consumers behave
// identically.
//
// Two-layer trust model (see feat/safe-offer-board / PR): the swap module's
// receive-side adapter already drops MALFORMED offers and enforces cache caps
// (swapDeliveryOfferIsWellFormed + per-maker fairness). This render-layer pass
// re-validates (never trust the input), then classifies each surviving offer:
//
//   * MALFORMED (bad hex, non-positive amount, NaN/negative/zero timelock,
//     already expired) -> dropped SILENTLY (noise, not a showcase).
//   * VENUE-MISMATCH (well-formed but a non-canonical eth_htlc_address /
//     lez_htlc_program_id) -> admitted as a GHOST row (visible, disabled
//     "blocked - unsafe"), capped at maxGhostRows; beyond the cap, dropped
//     silently so a flood can't bury honest offers.
//   * CANONICAL -> admitted as a normal, acceptable offer.
//
// The accept-time venue check in the plugin remains the true gate regardless;
// this is defense-in-depth applied earlier, for UX and spam control.

// Canonical-hex normalisation: strip an optional 0x, trim, lowercase. Mirrors
// the backend's swap_ui::normalizeHex so the board's verdict matches the
// accept-time check byte-for-byte.
function normHex(s) {
    if (s === undefined || s === null) return "";
    var h = String(s).trim();
    if (h.indexOf("0x") === 0 || h.indexOf("0X") === 0) h = h.substring(2);
    return h.toLowerCase();
}

// Strict hex-shape test: exactly nBytes*2 hex chars (0x optional).
function isHexBytes(s, nBytes) {
    var h = normHex(s);
    return h.length === nBytes * 2 && /^[0-9a-f]+$/.test(h);
}

// MALFORMED filter (review P1s: type-validation + NaN-timelock). Mirrors the
// backend's swapDeliveryOfferIsWellFormed.
function offerWellFormed(o) {
    if (!o) return false;
    if (!isHexBytes(o.maker_eth_address, 20)) return false;
    if (!isHexBytes(o.eth_htlc_address, 20)) return false;
    if (!isHexBytes(o.lez_htlc_program_id, 32)) return false;
    if (!(Number(o.lez_amount) > 0)) return false;
    if (!(Number(o.eth_amount) > 0)) return false;
    if (!(Number(o.lez_timelock) > 0)) return false;
    if (!(Number(o.eth_timelock) > 0)) return false;
    return true;
}

// VENUE filter. True iff the offer names the app's canonical pinned venue. A
// canonical value not yet resolved (empty) is treated as "unknown" -> no
// ghosting (the accept-time check remains the gate), never a mismatch.
function offerVenueOk(o, canonicalEth, canonicalLez) {
    var canEth = normHex(canonicalEth);
    var canLez = normHex(canonicalLez);
    if (canEth !== "" && normHex(o.eth_htlc_address) !== canEth) return false;
    if (canLez !== "" && normHex(o.lez_htlc_program_id) !== canLez) return false;
    return true;
}

// Absolute expiry (unix seconds) = the sooner of the two timelocks.
function offerExpirySec(o) {
    return Math.min(Number(o.lez_timelock), Number(o.eth_timelock));
}

// Classify a freshly-fetched batch against current board state. PURE: returns
// the list of offers to ADMIT (each tagged blocked true/false), in input
// order, and mutates nothing. Everything not returned was dropped silently
// (malformed / expired / duplicate / over a cap).
//
// ctx = {
//   nowSec, canonicalEth, canonicalLez,
//   existingKeys: [string],      // keys already on the board
//   ghostCount, honestCount,     // current counts on the board
//   maxOffers, maxGhostRows,     // caps
//   keyOf: function(offer)       // stable de-dupe key
// }
function classifyOffers(offers, ctx) {
    var admit = [];
    if (!offers) return admit;
    var seen = (ctx.existingKeys || []).slice();
    var ghosts = ctx.ghostCount || 0;
    var honest = ctx.honestCount || 0;
    for (var i = 0; i < offers.length; i++) {
        var o = offers[i];
        if (!offerWellFormed(o)) continue;                 // malformed -> drop
        if (ctx.nowSec >= offerExpirySec(o)) continue;     // already expired
        var key = ctx.keyOf(o);
        if (seen.indexOf(key) !== -1) continue;            // duplicate
        var blocked = !offerVenueOk(o, ctx.canonicalEth, ctx.canonicalLez);
        if (blocked) {
            if (ghosts >= ctx.maxGhostRows) continue;      // ghost cap -> drop
            ghosts++;
        } else {
            if (honest >= ctx.maxOffers) continue;         // spam cap -> drop
            honest++;
        }
        seen.push(key);
        admit.push({ offer: o, key: key, blocked: blocked });
    }
    return admit;
}

// CommonJS export for the node test's evaluator; harmlessly skipped under QML
// (where `module` is undefined). QML consumers call OfferFilter.<fn> directly.
if (typeof module !== "undefined" && module.exports) {
    module.exports = {
        normHex: normHex,
        isHexBytes: isHexBytes,
        offerWellFormed: offerWellFormed,
        offerVenueOk: offerVenueOk,
        offerExpirySec: offerExpirySec,
        classifyOffers: classifyOffers
    };
}
