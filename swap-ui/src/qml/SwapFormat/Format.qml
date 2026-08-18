pragma Singleton
import QtQuick

// The app's one formatting voice for on-wire values: amounts, rates, ages and
// hex identifiers.
//
// Every function here previously existed as a per-view copy — weiToEth in five
// files, fmtRate in two, timeAgo in two — and the hex truncation existed as one
// real helper (private to OfferBoard, called once) plus nine hand-rolled
// substring expressions with six different head/tail shapes and two different
// ellipsis characters. Formatting drift on a trust surface is not cosmetic: two
// screens showing the same address clipped differently reads as two different
// addresses.
QtObject {
    id: fmt

    // --- Amounts --------------------------------------------------------
    // Wei -> a display string, always in ETH. Deliberately lossy and for
    // display only; never round-trip a value through this.
    //
    // Always ETH — never Gwei or raw wei — so every amount on screen reads
    // in the same unit. Uses up to 8 decimal places (trailing zeros
    // trimmed) so small-but-real amounts stay visibly nonzero: a 4500 Gwei
    // offer (0.0000045 ETH) must never render as "0.00 ETH".
    function weiToEth(wei) {
        var n = Number(wei)
        if (isNaN(n) || n === 0) return "0 ETH"
        var eth = n / 1e18
        return eth.toFixed(8).replace(/\.?0+$/, '') + " ETH"
    }

    // Wei -> a bare numeric ETH string, for pre-filling config fields. No unit
    // suffix, no unit switching — the value has to survive being parsed back.
    function weiToEthValue(wei) {
        var n = Number(wei)
        if (isNaN(n) || n === 0) return "0"
        return (n / 1e18).toString()
    }

    // --- Rates ----------------------------------------------------------
    function fmtRate(r) {
        if (!(r > 0)) return "—"
        if (r >= 100) return r.toFixed(0)
        if (r >= 1) return r.toFixed(2)
        return r.toFixed(6)
    }

    // --- Time -----------------------------------------------------------
    function timeAgo(timestampMs) {
        if (!timestampMs) return ""
        var diff = Date.now() - timestampMs
        if (diff < 0) diff = 0
        var sec = Math.floor(diff / 1000)
        if (sec < 60) return sec + "s ago"
        var min = Math.floor(sec / 60)
        if (min < 60) return min + "m ago"
        var hr = Math.floor(min / 60)
        return hr + "h " + (min % 60) + "m ago"
    }

    // Time remaining until an absolute Unix timestamp (in SECONDS, not ms —
    // that is the unit the offer wire format uses for timelocks). Returns ""
    // for a missing deadline and "expired" once it has passed, so callers can
    // distinguish "no deadline known" from "deadline gone" and never invent a
    // countdown they cannot back with a real chain value.
    function expiresIn(timelockSec) {
        if (!timelockSec) return ""
        var diff = timelockSec - Math.floor(Date.now() / 1000)
        if (diff <= 0) return "expired"
        var min = Math.floor(diff / 60)
        if (min < 60) return min + "m"
        var hr = Math.floor(min / 60)
        return hr + "h " + (min % 60) + "m"
    }

    // --- Hex identifiers -------------------------------------------------
    // One truncation rule. Defaults suit a detail row: `head` of 8 keeps the
    // "0x" plus six meaningful characters, `tail` of 6 keeps enough to compare
    // against a block explorer by eye. Tight table cells pass (6, 4).
    //
    // Returns the input untouched when it is already short enough to show in
    // full, so a truncated value on screen always means "there is more".
    // Always U+2026, never three ASCII dots.
    function shortHex(s, head, tail) {
        var h = (head === undefined) ? 8 : head
        var t = (tail === undefined) ? 6 : tail
        if (!s) return "—"
        var str = String(s)
        if (str.length <= h + t + 1) return str
        return str.substring(0, h) + "…" + str.substring(str.length - t)
    }

    // True when shortHex would actually drop characters — lets a caller decide
    // whether a "show full value" affordance is worth offering.
    function isTruncated(s, head, tail) {
        var h = (head === undefined) ? 8 : head
        var t = (tail === undefined) ? 6 : tail
        return !!s && String(s).length > h + t + 1
    }
}
