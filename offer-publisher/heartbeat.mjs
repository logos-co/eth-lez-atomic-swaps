// The one offer-republish heartbeat knob, resolved in one place.
//
// `OFFER_HEARTBEAT_SECS` is the single source of truth for the republish
// cadence, end to end: `swap-cli maker` reads it, resolves it, and injects the
// RESOLVED value into this sidecar's env (see src/cli/maker.rs and
// src/cli/bot.rs). `FALLBACK_HEARTBEAT_SECS` is a DEPRECATED alias kept working
// for deployments that already set it — it warns at startup and will be
// removed.
//
// Note on 0: `swap-cli maker --heartbeat-secs 0` disables offer publishing by
// NOT spawning this sidecar at all, so 0 never reaches here as a cadence. A 0
// (or otherwise unusable) value in this process's env is therefore a mistake,
// not "disabled" — it is rejected with a warning rather than turned into
// `setInterval(fn, 0)`.
//
// Kept pure and separate from publish-offer.mjs so heartbeat.test.mjs can pin
// the precedence rules without a Waku node (see AGENTS.md, "Fast local
// checks").

/** Default republish cadence, in seconds. Must match `DEFAULT_HEARTBEAT_SECS`
 * in src/cli/maker.rs — a maker started without the env var and a sidecar run
 * standalone have to agree on the same cadence. */
export const DEFAULT_HEARTBEAT_SECS = 30;

export const CANONICAL_ENV = "OFFER_HEARTBEAT_SECS";
export const DEPRECATED_ENV = "FALLBACK_HEARTBEAT_SECS";

/**
 * Resolve the effective heartbeat interval from an env-like object.
 *
 * Precedence: the canonical var wins whenever it holds a usable value, the
 * deprecated alias is only consulted when it does not, and the default applies
 * when neither does. Values that are not positive integers are rejected (never
 * silently coerced): `setInterval(fn, NaN)` degenerates into a busy 1ms
 * republish loop, which is worse than any cadence an operator could have meant.
 *
 * @param {Record<string, string|undefined>} env
 * @returns {{secs: number, source: string, warnings: string[]}}
 *   `source` names the env var the value came from (or "default"); `warnings`
 *   are deprecation / ignored-value lines the caller MUST log at startup.
 */
export function resolveHeartbeatSecs(env = {}) {
  const warnings = [];
  const canonicalRaw = env[CANONICAL_ENV];
  const aliasRaw = env[DEPRECATED_ENV];
  const canonical = parsePositiveInt(canonicalRaw);
  const alias = parsePositiveInt(aliasRaw);

  if (isSet(canonicalRaw) && canonical === null) {
    warnings.push(
      `${CANONICAL_ENV}='${canonicalRaw}' is not a positive integer — ignored`
    );
  }
  if (isSet(aliasRaw)) {
    if (alias === null) {
      warnings.push(
        `${DEPRECATED_ENV}='${aliasRaw}' is not a positive integer — ignored ` +
          `(and ${DEPRECATED_ENV} itself is deprecated: use ${CANONICAL_ENV})`
      );
    } else if (canonical !== null) {
      warnings.push(
        `${DEPRECATED_ENV} is DEPRECATED and was IGNORED here — ` +
          `${CANONICAL_ENV}=${canonical} wins; drop ${DEPRECATED_ENV}`
      );
    } else {
      warnings.push(
        `${DEPRECATED_ENV} is DEPRECATED — rename it to ${CANONICAL_ENV} ` +
          `(still honoured: heartbeat ${alias}s)`
      );
    }
  }

  if (canonical !== null) {
    return { secs: canonical, source: CANONICAL_ENV, warnings };
  }
  if (alias !== null) {
    return { secs: alias, source: DEPRECATED_ENV, warnings };
  }
  return { secs: DEFAULT_HEARTBEAT_SECS, source: "default", warnings };
}

function isSet(raw) {
  return raw !== undefined && raw !== null && String(raw).trim() !== "";
}

function parsePositiveInt(raw) {
  if (!isSet(raw)) {
    return null;
  }
  const n = Number(String(raw).trim());
  if (!Number.isInteger(n) || n <= 0) {
    return null;
  }
  return n;
}
