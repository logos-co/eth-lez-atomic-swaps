# Offer Board — Design Variants (runners-up)

The live offer board (home screen, `swap-ui/src/qml/OfferBoard.qml`) was chosen
via a three-way design shotgun. Three variants were specced in parallel and
judged on three lenses: first-impression wow, usability-to-accept-an-offer,
and implementability in the existing plain-Qt-Quick codebase.

| Lens | A — Terminal Ticker | B — Card Market | C — Trading Dashboard |
|---|---|---|---|
| First-impression wow | 6 | 8 | 6 |
| Usability-to-accept | 7 | 8 | 9 |
| Implementability | 5 | 3 | 8 |
| **Total** | **18** | **19** | **23 (winner)** |

**Why C won:** it is the only variant whose live-market behaviors survive the
app's model idiom (offers merged into a keyed model — Repeater/ListView
delegates must not be wholesale-recreated every poll), and its persistent
detail pane doubles as the trust/confirmation surface (hashlock, HTLC
addresses, both timelock countdowns) for a two-click accept. Grafts taken from
the runners-up into the shipped implementation: the poll-drain heartbeat bar
and best-rate highlight (from A), the radar-ping freshness dot, the
"accepting…" button state, and the full-bleed empty state (from B).

**Why the others lost:** A's density aesthetic collapses at the realistic 0–3
offer volume, and its animation system assumed Repeater diffs a reassigned JS
array (it does not — all delegates are destroyed and recreated). B had the
best zero-state and guard rails, but its 1-second `offers.slice()` tick would
rebuild every delegate each second, destroying the in-card confirm overlay its
accept flow depends on. Both are recoverable designs if the model layer below
(keyed ListModel merge, as shipped) is kept.

The full runner-up specs follow, preserved verbatim so switching directions
later is cheap. Their QML sketches assume the same `swapBackend` surface the
shipped board uses; re-mount them on the shipped ListModel/merge/tick
infrastructure rather than their own model-handling notes.

---

# Variant A — Terminal Ticker

I have full context on the app's idioms, helpers, and bindings. Here is the complete design spec.

---

# TERMINAL TICKER — Live Offer Board (swap_ui HOME)

Design language: a financial order book. Dark, dense, monospace numerics, right-aligned columns that align digit-for-digit, live-updating rows that fade with age and strike-through on expiry. Orange (`accent`) is reserved for "actionable now" and price emphasis — everything else is greyscale so the eye reads the tape, not the chrome.

---

## 1. Layout & hierarchy

Root is an `Item` filling the tab. Three fixed bands (status strip, title/controls, column header) pinned to the top, and ONE scrolling region (the tape) that fills the remainder. A one-line footer is pinned to the bottom. Only the tape scrolls.

```
┌───────────────────────────────────────────────────────────────────────────┐
│ ● LIVE   peers 4   ⛓ ETH 0.842   ◆ LEZ 1,200   config ✓        auto 3s ▮  │ ← STATUS STRIP (fixed, 32px)
├───────────────────────────────────────────────────────────────────────────┤
│ OFFER BOARD                                          [ ⟳ Refresh ]  [+ Sell]│ ← TITLE ROW (fixed)
├───────────────────────────────────────────────────────────────────────────┤
│ SELL LEZ        BUY (ETH)      PRICE LEZ/ETH   MAKER      EXPIRES      AGE   │ ← COLUMN HEADER (fixed, monospace, textMuted, uppercase)
├───────────────────────────────────────────────────────────────────────────┤ ↑ hairline border
│▎ 1,200 LEZ      0.842000 ETH      1,425.17     0x9f3a…    12m / 14m    3s   │ ┐
│▎   500 LEZ      0.350000 ETH      1,428.57     0x71bd…     8m / 10m   11s   │ │
│   2,000 LEZ     1.500000 ETH      1,333.33     0x0af2…     4m /  6m    47s  │ │  TAPE
│   800 LEZ       0.640000 ETH      1,250.00     0x3c19…     2m /  3m   2m ago│ │ (scrolls,
│   300 LEZ       0.240000 ETH      1,250.00     0xbe07…    EXPIRED     5m ago│ │  age-fades
│   ...                                                                       │ ┘  downward)
├───────────────────────────────────────────────────────────────────────────┤
│ 5 live · 1 expiring · showing freshest first · last drain 2s ago            │ ← FOOTER (fixed, 22px, textMuted)
└───────────────────────────────────────────────────────────────────────────┘
```

- **Fixed:** status strip, title row, column header, footer.
- **Scrolling:** the tape (`Flickable` + `Repeater`), `clip: true`.
- `▎` on a row = the accent left-edge marker that appears on hover/selection (the "cursor" in the order book).
- Newest offers insert at the **top**; the list is sorted `timestamp_ms` descending so the tape reads like a real feed (freshest at the top, decaying downward).

---

## 2. The order-book row

Fixed height **36px** per row (dense; no card padding, no per-row radius — rows are tape lines separated by a 1px `border` hairline, not floating cards). One `RowLayout` of `Text` columns over a full-bleed `Rectangle`.

**Columns (left→right), with alignment and font:**

| # | Column | Content | Align | Font | Color token |
|---|--------|---------|-------|------|-------------|
| 1 | SELL LEZ | `lez_amount` + ` LEZ`, thousands-grouped | right (number), unit left | Menlo 15 (`fontNormal`) | `textPrimary` |
| 2 | BUY (ETH) | `weiToEth(eth_amount)` → e.g. `0.842000 ETH` | right | Menlo 15 | `textPrimary` |
| 3 | PRICE LEZ/ETH | derived ratio (see below), 2 decimals, grouped | right | Menlo 15 **bold** | `accent` |
| 4 | MAKER | `maker_eth_address` → `0x9f3a…` (6-char head + ellipsis) | left | Menlo 13 (`fontSmall`) | `textSecondary` |
| 5 | EXPIRES | `expiresIn(lez_timelock)` + ` / ` + `expiresIn(eth_timelock)` | right | Menlo 13 | traffic-light (see §3) |
| 6 | AGE | `timeAgo(timestamp_ms)` → `3s` / `2m ago` | right | Menlo 11 | `textMuted` |

**Price derivation (the headline number).** Show **LEZ per ETH** — "how much LEZ you get per 1 ETH" — because the taker is buying LEZ and wants to compare rates, and bigger = better for the buyer:

```js
function priceLezPerEth(o) {
    var eth = Number(o.eth_amount) / 1e18       // wei → ETH
    var lez = Number(o.lez_amount)
    if (!eth || isNaN(eth)) return "—"
    return group((lez / eth).toFixed(2))         // group() adds thousands separators
}
```

Best (highest LEZ/ETH) is not re-sorted to the top — sort stays chronological so the tape feels live — but the best-price row gets a subtle `success`-tinted price cell (see §4) so a scanner spots the deal.

**Shown vs hidden.** Shown: the 6 columns above. Hidden on the row (revealed only in the confirm overlay, §7): full `maker_eth_address`, `maker_lez_account`, `hashlock`, `lez_htlc_program_id`, `eth_htlc_address`, and the numeric absolute timelocks. The row is a quote; identity/contract detail is confirm-time detail.

**Hover / selected affordance.**
- Default: row `color: transparent` over the `background`; text at its age-faded opacity (§3).
- Hover (`MouseArea.containsMouse`): row bg → `surface`; a 3px `accent` bar appears at the left edge (the `▎` cursor) via width animation; `cursorShape: Qt.PointingHandCursor`; the AGE cell is replaced by a compact **`▸ ACCEPT`** hint in `accent`.
- Selected/pending: left bar stays, border tint → `accent`, and the inline confirm expands (§7).

**One-tap accept affordance.** The whole row is the click target (`MouseArea` fills it). A single click selects the row and reveals the inline confirm strip directly beneath it (accordion) — no full-screen modal, keeps the tape visible. Expired/expiring-past-timelock rows are `enabled: false` (not clickable).

---

## 3. Live behaviors

**Auto-refresh Timer + merge/dedup.** A `Timer` (`interval: 3000`, `repeat`, `running: messagingConnected && !running`) calls `swapBackend.fetchOffers()`. Because `fetchOffers()` is a **destructive relay drain**, results are merged client-side, reusing the exact key already used in TakerView:

```js
key = o.maker_eth_address + ":" + o.lez_amount + ":" + o.eth_amount
```

Merge rule on each `onOffersFetched`:
- New key → **insert** at correct chronological position (unshift-then-sort by `timestamp_ms` desc). Triggers the insert animation.
- Existing key → **refresh** `timestamp_ms` to the new drain time (re-freshens age) and keep it.
- The model is a JS array reassigned wholesale (`discoveredOffers = merged`) so the `Repeater` diffs — same idiom as TakerView.

**Pause during a swap:** when `swapBackend.running` the Timer stops (avoid draining relay mid-swap); footer notes "paused — swap in progress".

**New-offer insert animation.** New rows appear with `opacity 0 → 1` and a slide: `y` offset animates from `-8px` to rest, plus a 400ms `accent`-tinted background flash that decays to transparent (the "new print" highlight on a tape). Implemented with `add` `Transition` on a positioner, or `Behavior on opacity` + a one-shot `flash` Timer per delegate.

**Age-fade.** Row opacity is a live function of `timestamp_ms`. A single 1s "clock" `Timer` bumps a `property real now: Date.now()` that all rows bind to, so fades update without per-row timers:

```
ageMs = now - timestamp_ms
opacity = ageMs < 30_000  ? 1.0
        : ageMs < 120_000 ? 0.85
        : ageMs < 300_000 ? 0.6
        :                   0.4      // floor
```

`Behavior on opacity { NumberAnimation { duration: 600 } }` so fades glide.

**TTL / expiry handling.** Expiry is driven by the timelocks, not age. Compute `minTimelock = Math.min(lez_timelock, eth_timelock)` and `secsLeft = minTimelock - now/1000`:
- `secsLeft <= 300` (5m) → **expiring:** EXPIRES cell → `warning`, cell pulses (opacity 0.6↔1.0 loop).
- `secsLeft <= 60` → EXPIRES → `error`.
- `secsLeft <= 0` → **expired:** whole row text → `textMuted`, EXPIRES shows `EXPIRED` in `error`, `font.strikeout: true` on amount cells, row `enabled: false`. After a **6s** grace (so the user sees the strike), a prune pass drops it with an `opacity → 0` + `height → 0` collapse animation, then removes the key from the model.

**Freshness countdown.** The status strip shows a live `auto 3s ▮` where a thin `accent` bar (`Rectangle` width bound to `nextDrainMs/interval`) drains left-to-right between refreshes, giving a heartbeat that the feed is live. Resets on each `fetchOffers()`.

**Pruning strategy.** On every clock tick: (a) collapse+remove offers `secsLeft <= -6`; (b) cap the tape at the freshest **50** offers (drop the tail) to keep it dense and bounded.

---

## 4. Color & typography → exact tokens

Typography: **all numeric columns are `font.family: "Menlo, Courier New"`** so digits align in a monospace grid — this is the whole point of the terminal look. Labels/headers use the same monospace, uppercased, for the tape feel.

- Screen bg: `Theme.background` (#171717). Status strip / footer bg: `Theme.surface` (#1E1E1E). Hovered row bg: `Theme.surface`; header bg: `Theme.inputBackground` (#1A1A1A) to seat it below the controls.
- Hairlines between rows, header underline, footer top-border: `Theme.border` (#333333), 1px.
- SELL/BUY amounts: `Theme.textPrimary` (#F0F0F0), `fontNormal` 15.
- **PRICE cell: `Theme.accent` (#FF8800), bold** — the one place orange lives in a resting row. Best-price row's price cell: `Theme.success` (#4ecca3).
- MAKER: `Theme.textSecondary` (#999999), `fontSmall` 13.
- Column header labels: `Theme.textMuted` (#666666), `fontSmall` 13, uppercase.
- AGE + footer: `Theme.textMuted` (#666666), 11px.
- EXPIRES traffic light: healthy `Theme.textSecondary` → expiring `Theme.warning` (#f9a826) → critical/expired `Theme.error` (#e94560).
- Hover left-edge cursor + inline ACCEPT + selected border: `Theme.accent`; accept button hover: `Theme.accentHover` (#FF9922).
- New-print flash: `Theme.accent` at low alpha decaying to transparent.
- Status strip LIVE dot: `Theme.success`; connecting/retry: `Theme.warning`; disconnected: `Theme.error`.
- Radius: rows are square (radius 0) for tape density; the status strip, controls, and inline confirm use `radiusSmall` 6 / `radiusNormal` 8. Spacing between columns: `spacingLarge` 24 (generous inter-column gutters keep the grid legible); band paddings: `spacingNormal` 16.

---

## 5. Empty state & connection state

Driven by `messaging*` bindings. Precedence: disconnected → connecting → empty → tape.

- **Disconnected** (`!messagingConnected && !messagingRetrying`): centered in the tape area — big muted glyph, `"Tape offline"` (`fontLarge`, `textSecondary`), sub `"Delivery is starting automatically."` (`textMuted`). Status dot `error`.
- **Connecting / retrying** (`messagingLoading || messagingRetrying`): `"Connecting to the market…"` in `Theme.warning`, an indeterminate 3-dot ellipsis animation, peer count if any. Status dot `warning` pulsing.
- **Empty but connected** (`discoveredOffers.length === 0 && !offersLoading`): **"Market waking up"** headline (`fontLarge`, `textPrimary`), sub `"No live offers on the tape yet — the board is listening."` (`textMuted`), a faint animated single scanning line across the empty grid to signal liveness, and a **maker CTA button** `"+ Post an offer to sell LEZ"` (accent-outline Button, idiom from TakerView's Discover button) that switches to the Maker tab. A secondary ghost `"⟳ Drain now"` triggers `fetchOffers()` immediately.
- **First load** (`offersLoading && length===0`): `"Draining relay…"` with the freshness bar animating.

---

## 6. Connection + balances + config-readiness status strip

A single fixed 32px `RowLayout` at the very top (`surface` bg, `border` bottom hairline), left→right, all monospace `fontSmall`:

1. **Live dot + label:** `● LIVE` (`success`) when `messagingConnected`; `● CONNECTING` (`warning`) when `messagingLoading || messagingRetrying`; `● OFFLINE` (`error`) otherwise. Text mirrors `messagingConnectionStatus`.
2. **Peers:** `peers {messagingPeerCount}` — `textSecondary`, or `textMuted` if 0.
3. **⛓ ETH balance:** `weiToEth(ethBalance)` — `textPrimary`; `ethAddress` head shown as a muted `0x…` chip.
4. **◆ LEZ balance:** `lezBalance` LEZ — `textPrimary`; `lezAccount` head muted.
5. **config ✓ / config ⚠:** parse `validationErrorsJson` — if `{}`/empty → `config ✓` in `success`; else `config ⚠ (n)` in `warning`, clickable → Config tab. This is the config-readiness signal; accept is disabled when config is not ✓.
6. **Right-aligned freshness meter:** `auto {n}s` + the draining `accent` bar (§3).

`Item { Layout.fillWidth: true }` spacers separate the left cluster from the right freshness meter. Everything greyscale except the semantic dot, the config flag, and the freshness bar.

---

## 7. Accept interaction (tap → confirm → start → hand-off)

1. **Tap row** → `pendingOffer = modelData`; the row gets the accent left-bar + border; an **inline confirm strip** accordions open directly beneath the row (`Behavior on height`), keeping the tape context.
2. **Confirm strip** (bg `surface`, border `accent`, `radiusNormal`) shows the resolved trade in plain language plus the fields hidden from the row:
   - `Buy {lez_amount} LEZ for {weiToEth(eth_amount)}` (bold, `textPrimary`).
   - Price `{priceLezPerEth} LEZ/ETH` (accent).
   - `from {maker_eth_address 6…4}` (`textSecondary`, monospace).
   - Warning line (`warning`): `"Starting locks your ETH and waits for the maker to lock LEZ. Completes only if the maker is live."` (mirrors existing TakerView copy).
   - Guard line if `secsLeft` small: `"⚠ Expires in {expiresIn(min)} — start now or it may lapse."`
   - Two buttons (TakerView Button idiom): **`Buy`** (filled `accent`/`accentHover`, white text, `disabled` while `running` or config not ✓) and **`Cancel`** (ghost, `border`).
3. **Buy** → `acceptedOffer = pendingOffer; pendingOffer = null; swapBackend.acceptOfferAndStartTaker(offer)`. Pause the refresh Timer.
4. **Hand-off:** on the call, emit a `requestTab("taker")` signal the shell listens to, switching the app to the **Taker progress tab** where the `ProgressStepper` drives from `takerCurrentStep`. The board itself returns to the tape on next visit; `onTakerRunningChanged` clears `acceptedOffer` when the swap ends (existing pattern).

---

## 8. QML sketch (core structure)

```qml
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import SwapTheme

Item {
    id: boardRoot
    signal requestTab(string tab)

    property var offers: []              // JS array model, sorted by timestamp_ms desc
    property var pendingOffer: null
    property real now: Date.now()        // shared clock for age/expiry bindings
    property real bestPrice: 0

    // ---- helpers (weiToEth / timeAgo / expiresIn mirror TakerView) ----
    function weiToEth(wei) {
        var n = Number(wei); if (isNaN(n) || n === 0) return "0 ETH"
        var eth = n / 1e18
        return (eth >= 0.001 ? eth.toFixed(6).replace(/\.?0+$/, '') : (n/1e9).toFixed(4)) + " ETH"
    }
    function group(s) { return s.replace(/\B(?=(\d{3})+(?!\d))/g, ",") }
    function timeAgo(ms) {
        var d = Math.max(0, now - ms), s = Math.floor(d/1000)
        if (s < 60) return s + "s"; var m = Math.floor(s/60)
        return m < 60 ? m + "m ago" : Math.floor(m/60) + "h " + (m%60) + "m"
    }
    function expiresIn(sec) {
        var d = sec - Math.floor(now/1000); if (d <= 0) return "expired"
        var m = Math.floor(d/60); return m < 60 ? m+"m" : Math.floor(m/60)+"h "+(m%60)+"m"
    }
    function priceLezPerEth(o) {
        var eth = Number(o.eth_amount)/1e18, lez = Number(o.lez_amount)
        return (!eth || isNaN(eth)) ? "—" : group((lez/eth).toFixed(2))
    }
    function minTimelock(o) { return Math.min(Number(o.lez_timelock), Number(o.eth_timelock)) }
    function secsLeft(o)    { return minTimelock(o) - now/1000 }
    function rowOpacity(ms) {
        var a = now - ms
        return a < 30000 ? 1.0 : a < 120000 ? 0.85 : a < 300000 ? 0.6 : 0.4
    }

    // ---- live clock: drives age-fade, countdowns, and prune pass ----
    Timer {
        interval: 1000; running: true; repeat: true
        onTriggered: {
            boardRoot.now = Date.now()
            // prune expired-past-grace + cap at 50
            var live = offers.filter(function(o){ return boardRoot.secsLeft(o) > -6 })
            if (live.length > 50) live = live.slice(0, 50)
            if (live.length !== offers.length) offers = live
        }
    }

    // ---- destructive-drain auto refresh, merge + dedup ----
    Timer {
        interval: 3000; repeat: true
        running: swapBackend.messagingConnected && !swapBackend.running
        onTriggered: swapBackend.fetchOffers()
    }
    Connections {
        target: swapBackend
        function onOffersFetched(json) {
            var obj = {}; try { obj = JSON.parse(json || "{}") } catch(e) { return }
            if (!obj.offers) return
            var merged = boardRoot.offers.slice(), seen = {}
            for (var i=0;i<merged.length;i++)
                seen[merged[i].maker_eth_address+":"+merged[i].lez_amount+":"+merged[i].eth_amount] = i
            for (var j=0;j<obj.offers.length;j++) {
                var o = obj.offers[j], k = o.maker_eth_address+":"+o.lez_amount+":"+o.eth_amount
                if (k in seen) merged[seen[k]].timestamp_ms = o.timestamp_ms  // refresh
                else merged.push(o)                                          // new print
            }
            merged.sort(function(a,b){ return b.timestamp_ms - a.timestamp_ms })
            var best = 0
            for (var m=0;m<merged.length;m++)
                best = Math.max(best, Number(merged[m].lez_amount)/(Number(merged[m].eth_amount)/1e18) || 0)
            boardRoot.bestPrice = best
            boardRoot.offers = merged
        }
    }

    Rectangle { anchors.fill: parent; color: Theme.background }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // ============ STATUS STRIP (fixed) ============
        Rectangle {
            Layout.fillWidth: true; Layout.preferredHeight: 32
            color: Theme.surface
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.border }
            RowLayout {
                anchors { fill: parent; leftMargin: Theme.spacingNormal; rightMargin: Theme.spacingNormal }
                spacing: Theme.spacingLarge
                Text {
                    text: (swapBackend.messagingConnected ? "● LIVE"
                          : (swapBackend.messagingLoading || swapBackend.messagingRetrying) ? "● CONNECTING" : "● OFFLINE")
                    color: swapBackend.messagingConnected ? Theme.success
                          : (swapBackend.messagingRetrying || swapBackend.messagingLoading) ? Theme.warning : Theme.error
                    font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall; bold: true }
                }
                Text { text: "peers " + swapBackend.messagingPeerCount
                       color: swapBackend.messagingPeerCount>0 ? Theme.textSecondary : Theme.textMuted
                       font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall } }
                Text { text: "⛓ " + boardRoot.weiToEth(swapBackend.ethBalance)
                       color: Theme.textPrimary; font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall } }
                Text { text: "◆ " + swapBackend.lezBalance + " LEZ"
                       color: Theme.textPrimary; font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall } }
                Text {
                    property bool ok: (swapBackend.validationErrorsJson || "{}").replace(/\s/g,"") === "{}"
                    text: ok ? "config ✓" : "config ⚠"
                    color: ok ? Theme.success : Theme.warning
                    font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall }
                    MouseArea { anchors.fill: parent; enabled: !parent.ok
                        cursorShape: Qt.PointingHandCursor; onClicked: boardRoot.requestTab("config") }
                }
                Item { Layout.fillWidth: true }
                // freshness meter
                Item {
                    Layout.preferredWidth: 60; Layout.preferredHeight: 4
                    Rectangle { anchors.fill: parent; color: Theme.inputBackground; radius: 2 }
                    Rectangle {
                        id: freshBar; height: parent.height; radius: 2; color: Theme.accent; width: parent.width
                        NumberAnimation on width { running: true; loops: Animation.Infinite
                            from: parent ? parent.width : 60; to: 0; duration: 3000 }
                    }
                }
            }
        }

        // ============ TITLE + CONTROLS (fixed) ============
        RowLayout {
            Layout.fillWidth: true
            Layout.margins: Theme.spacingNormal
            Text { text: "OFFER BOARD"; color: Theme.textPrimary
                   font { family: "Menlo, Courier New"; pixelSize: Theme.fontLarge; bold: true } }
            Item { Layout.fillWidth: true }
            Button {
                text: swapBackend.offersLoading ? "Draining…" : "⟳ Refresh"
                enabled: swapBackend.messagingConnected && !swapBackend.offersLoading
                Layout.preferredHeight: 32
                background: Rectangle { color: parent.hovered ? Qt.darker(Theme.surface,1.1) : Theme.surface
                    border.color: Theme.accent; border.width: 1; radius: Theme.radiusNormal }
                contentItem: Text { text: parent.text; color: parent.enabled ? Theme.accent : Theme.textMuted
                    font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall }
                    horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                onClicked: swapBackend.fetchOffers()
            }
            Button {
                text: "+ Sell"; Layout.preferredHeight: 32; leftPadding: 12; rightPadding: 12
                background: Rectangle { color: parent.hovered ? Theme.accentHover : Theme.accent; radius: Theme.radiusNormal }
                contentItem: Text { text: parent.text; color: "#ffffff"
                    font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall; bold: true }
                    horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                onClicked: boardRoot.requestTab("maker")
            }
        }

        // ============ COLUMN HEADER (fixed) ============
        Rectangle {
            Layout.fillWidth: true; Layout.preferredHeight: 28; color: Theme.inputBackground
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.border }
            RowLayout {
                anchors { fill: parent; leftMargin: Theme.spacingNormal; rightMargin: Theme.spacingNormal }
                spacing: Theme.spacingLarge
                property var hdr: ["SELL LEZ","BUY (ETH)","PRICE LEZ/ETH","MAKER","EXPIRES","AGE"]
                Repeater { model: parent.hdr
                    Text { text: modelData; color: Theme.textMuted
                        Layout.fillWidth: index < 3
                        horizontalAlignment: index === 3 ? Text.AlignLeft : Text.AlignRight
                        font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall } } }
            }
        }

        // ============ TAPE (scrolls) ============
        Flickable {
            id: tape
            Layout.fillWidth: true; Layout.fillHeight: true
            clip: true; contentHeight: rowsCol.implicitHeight
            boundsBehavior: Flickable.StopAtBounds

            // ---- empty / connecting states ----
            ColumnLayout {
                anchors.centerIn: parent; width: parent.width * 0.7; spacing: Theme.spacingSmall
                visible: boardRoot.offers.length === 0
                Text {
                    horizontalAlignment: Text.AlignHCenter; Layout.fillWidth: true
                    text: !swapBackend.messagingConnected
                          ? (swapBackend.messagingRetrying || swapBackend.messagingLoading ? "Connecting to the market…" : "Tape offline")
                          : (swapBackend.offersLoading ? "Draining relay…" : "Market waking up")
                    color: swapBackend.messagingConnected ? Theme.textPrimary : Theme.warning
                    font { family: "Menlo, Courier New"; pixelSize: Theme.fontLarge } }
                Text {
                    horizontalAlignment: Text.AlignHCenter; Layout.fillWidth: true; wrapMode: Text.Wrap
                    text: swapBackend.messagingConnected
                          ? "No live offers on the tape yet — the board is listening."
                          : "Delivery is starting automatically."
                    color: Theme.textMuted; font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall } }
                Button {
                    visible: swapBackend.messagingConnected; Layout.alignment: Qt.AlignHCenter
                    Layout.topMargin: Theme.spacingNormal; Layout.preferredHeight: 40; padding: 16
                    text: "+ Post an offer to sell LEZ"
                    background: Rectangle { color: parent.hovered ? Qt.darker(Theme.surface,1.1) : Theme.surface
                        border.color: Theme.accent; border.width: 1; radius: Theme.radiusNormal }
                    contentItem: Text { text: parent.text; color: Theme.accent
                        font { family: "Menlo, Courier New"; pixelSize: Theme.fontNormal; bold: true }
                        horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                    onClicked: boardRoot.requestTab("maker") }
            }

            // ---- rows ----
            Column {
                id: rowsCol; width: tape.width
                add: Transition {                       // new-print insert animation
                    NumberAnimation { property: "opacity"; from: 0; to: 1; duration: 300 }
                    NumberAnimation { property: "y"; from: -8; duration: 300; easing.type: Easing.OutCubic }
                }
                Repeater {
                    model: boardRoot.offers
                    delegate: Rectangle {
                        id: row
                        width: rowsCol.width; height: 36
                        property bool expired: boardRoot.secsLeft(modelData) <= 0
                        property real left: boardRoot.secsLeft(modelData)
                        property bool best: (Number(modelData.lez_amount)/(Number(modelData.eth_amount)/1e18))
                                            >= boardRoot.bestPrice - 0.01 && boardRoot.bestPrice > 0
                        color: rowMouse.containsMouse ? Theme.surface : "transparent"
                        opacity: expired ? 0.5 : boardRoot.rowOpacity(modelData.timestamp_ms)
                        Behavior on opacity { NumberAnimation { duration: 600 } }
                        Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.border }
                        // accent cursor bar (hover/selected)
                        Rectangle {
                            anchors.left: parent.left; height: parent.height
                            width: (rowMouse.containsMouse || boardRoot.pendingOffer === modelData) ? 3 : 0
                            color: Theme.accent
                            Behavior on width { NumberAnimation { duration: 120 } }
                        }
                        // one-shot new-print flash
                        Rectangle {
                            id: flash; anchors.fill: parent; color: Theme.accent; opacity: 0
                            Component.onCompleted: flashAnim.start()
                            NumberAnimation { id: flashAnim; target: flash; property: "opacity"
                                from: 0.18; to: 0; duration: 500 }
                        }

                        MouseArea {
                            id: rowMouse; anchors.fill: parent; hoverEnabled: true
                            enabled: !row.expired && !swapBackend.running
                            cursorShape: Qt.PointingHandCursor
                            onClicked: boardRoot.pendingOffer = modelData
                        }

                        RowLayout {
                            anchors { fill: parent; leftMargin: Theme.spacingNormal; rightMargin: Theme.spacingNormal }
                            spacing: Theme.spacingLarge
                            Text { Layout.fillWidth: true; horizontalAlignment: Text.AlignRight
                                text: boardRoot.group(modelData.lez_amount) + " LEZ"
                                color: Theme.textPrimary; font.strikeout: row.expired
                                font { family: "Menlo, Courier New"; pixelSize: Theme.fontNormal } }
                            Text { Layout.fillWidth: true; horizontalAlignment: Text.AlignRight
                                text: boardRoot.weiToEth(modelData.eth_amount)
                                color: Theme.textPrimary; font.strikeout: row.expired
                                font { family: "Menlo, Courier New"; pixelSize: Theme.fontNormal } }
                            Text { Layout.fillWidth: true; horizontalAlignment: Text.AlignRight
                                text: boardRoot.priceLezPerEth(modelData)
                                color: row.best ? Theme.success : Theme.accent
                                font { family: "Menlo, Courier New"; pixelSize: Theme.fontNormal; bold: true } }
                            Text { Layout.preferredWidth: 90; horizontalAlignment: Text.AlignLeft
                                text: modelData.maker_eth_address.substring(0,6) + "…"
                                color: Theme.textSecondary; font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall } }
                            Text { Layout.preferredWidth: 120; horizontalAlignment: Text.AlignRight
                                text: row.expired ? "EXPIRED"
                                      : boardRoot.expiresIn(modelData.lez_timelock)+" / "+boardRoot.expiresIn(modelData.eth_timelock)
                                color: row.expired ? Theme.error : row.left <= 60 ? Theme.error
                                       : row.left <= 300 ? Theme.warning : Theme.textSecondary
                                font { family: "Menlo, Courier New"; pixelSize: Theme.fontSmall }
                                SequentialAnimation on opacity { running: row.left <= 300 && !row.expired
                                    loops: Animation.Infinite
                                    NumberAnimation { to: 0.6; duration: 600 } NumberAnimation { to: 1.0; duration: 600 } } }
                            Text { Layout.preferredWidth: 70; horizontalAlignment: Text.AlignRight
                                text: rowMouse.containsMouse && !row.expired ? "▸ ACCEPT" : boardRoot.timeAgo(modelData.timestamp_ms)
                                color: rowMouse.containsMouse && !row.expired ? Theme.accent : Theme.textMuted
                                font { family: "Menlo, Courier New"; pixelSize: 11
                                       bold: rowMouse.containsMouse } }
                        }
                        // ... inline confirm strip (accordion) renders below when pendingOffer === modelData:
                        //     Buy/Cancel buttons → swapBackend.acceptOfferAndStartTaker(modelData) + requestTab("taker")
                    }
                }
            }
        }

        // ============ FOOTER (fixed) ============
        Rectangle {
            Layout.fillWidth: true; Layout.preferredHeight: 22; color: Theme.surface
            Rectangle { anchors.top: parent.top; width: parent.width; height: 1; color: Theme.border }
            Text {
                anchors { verticalCenter: parent.verticalCenter; left: parent.left; leftMargin: Theme.spacingNormal }
                text: boardRoot.offers.length + " live · freshest first"
                      + (swapBackend.running ? " · paused (swap in progress)" : "")
                color: Theme.textMuted; font { family: "Menlo, Courier New"; pixelSize: 11 }
            }
        }
    }
}
```

**Notes for the builder.** The inline confirm strip (elided at the row's tail comment) reuses TakerView's confirm-card markup verbatim — same `acceptedOffer/pendingOffer` flow and Button styling — but rendered as an accordion under the selected row instead of a separate card, and on `Buy` it also emits `requestTab("taker")` so the shell switches tabs. The `bestPrice` compare uses a small epsilon so the top-rate row's price cell turns `success` green. All numeric columns share `"Menlo, Courier New"` so digits align into the order-book grid — this is the load-bearing detail of the whole aesthetic.

---

# Variant B — Card Market

# CARD MARKET — Live Offer Board (Home Screen) Design Spec

A tactile marketplace of live swap cards. Big pair/amount typography, a freshness pulse on brand-new offers, deterministic maker identicons, generous whitespace on `#171717`. Below is the complete, build-ready spec followed by an idiomatic QML sketch.

---

## 1. Layout & Hierarchy

Three horizontal bands. Header and status strip are **fixed** (pinned, never scroll). The card grid is the only **scrolling** region.

```
┌────────────────────────────────────────────────────────────────────────┐
│  LEZ ⇄ ETH MARKET                              [ ● Live · 4 peers ]      │ ← header (fixed, 72px)
│  Atomic swaps, settling in real time                                     │
├────────────────────────────────────────────────────────────────────────┤
│  ◈ ETH 0.42 · LEZ 1,204     │   Config ✓ ready   │   ⟳ refreshed 3s ago  │ ← status strip (fixed, 48px)
├────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│   ┌───────────────────┐  ┌───────────────────┐  ┌───────────────────┐   │
│   │ ◆ pulse   12m left│  │ ◆        1h 5m    │  │ ◆        44m      │   │ ← card grid
│   │                   │  │                   │  │                   │   │   (scrolls,
│   │   1,204 LEZ       │  │     850 LEZ       │  │    3,000 LEZ      │   │    responsive
│   │   for 0.42 ETH    │  │   for 0.30 ETH    │  │   for 1.05 ETH    │   │    columns)
│   │   ≈ 2,866 LEZ/ETH  │  │   ≈ 2,833 LEZ/ETH │  │  ≈ 2,857 LEZ/ETH  │   │
│   │                   │  │                   │  │                   │   │
│   │ (◑) 0x8f3a…c2e1   │  │ (◐) 0x11bb…9a4f   │  │ (◒) 0x77cd…0e2a   │   │
│   │       5s ago      │  │       48s ago     │  │      2m ago       │   │
│   │  ┌─────────────┐  │  │  ┌─────────────┐  │  │  ┌─────────────┐  │   │
│   │  │  Take offer │  │  │  │  Take offer │  │  │  │  Take offer │  │   │
│   │  └─────────────┘  │  │  └─────────────┘  │  │  └─────────────┘  │   │
│   └───────────────────┘  └───────────────────┘  └───────────────────┘   │
│                                                                          │
└────────────────────────────────────────────────────────────────────────┘
```

**Grid responsiveness.** Use a `Flow` (not fixed `GridLayout` column count) inside a `ScrollView`/`Flickable` so cards reflow by available width. Card is fixed width **300px**, fixed height **248px** (tactile, uniform tiles — a true "card market" reads as a regular lattice). Column count = `Math.max(1, Math.floor((width - spacingLarge) / (300 + spacingNormal)))`. Content is centered with `spacingLarge` (24) outer padding and `spacingNormal` (16) gutters. Uniform card height is deliberate: it lets insert/collapse animations play without reflow jank.

**Ordering.** Newest first (descending `timestamp_ms`). Freshest cards appear top-left, so the pulse energy lives where the eye lands.

---

## 2. Offer Card Anatomy

Card = `Rectangle`, width 300, height 248, `color: Theme.surface` (`#1E1E1E`), `radius: Theme.radiusLarge` (12), `border 1px Theme.border` (`#333333`). On hover the border brightens to `Theme.accent` and the card lifts (see §3).

Vertical anatomy, top → bottom, `spacingNormal` (16) internal padding:

1. **Top meta row** (RowLayout, height ~20): left = freshness dot (only when fresh, see §3); right = **expiry chip** (§ below). Space-between.

2. **Hero block** (the star). Two-line composition:
   - **Hero number = LEZ amount.** `lez_amount`, font **28px bold**, `Theme.textPrimary` (`#F0F0F0`), with a smaller " LEZ" unit suffix at `fontLarge` (18) `Theme.textSecondary`. LEZ is the hero because this board's identity is "LEZ market," and the taker is buying LEZ.
   - **Sub-line:** `"for " + weiToEth(eth_amount) + " ETH"` at `fontNormal` (15) `Theme.textSecondary` (`#999999`). The word "for" in `Theme.textMuted`.
   - **Rate line:** `"≈ " + rate + " LEZ/ETH"` at `fontSmall` (13) monospace `Theme.textMuted` (`#666666`). Computed `lez_amount / weiToEth(eth_amount)`, formatted to 0 decimals. The rate is the *scannability* anchor — the deal-comparison signal — but stays visually quiet under the hero.

3. **Maker row** (RowLayout, spacingSmall): **identicon** (see below) + a column with short-address (mono `fontNormal`, `Theme.textPrimary`) over `timeAgo(timestamp_ms)` (`fontSmall`, `Theme.textMuted`).

4. **Accept affordance:** an explicit **"Take offer"** Button (full card width, `inputHeight` 40, `radiusNormal`), background `Theme.surfaceLight` default → `Theme.accent` on hover, text `Theme.textPrimary` → `#171717` on hover. Explicit button, **not** whole-card tap: accepting spends real funds and hands off to a swap flow, so it must be a deliberate, unmistakable target. Whole-card hover is a *preview affordance* (lift + border glow) only.

**Short address:** `addr.slice(0,6) + "…" + addr.slice(-4)` → `0x8f3a…c2e1`, monospace "Menlo, Courier New".

**Identicon** (deterministic, no assets). 36×36 `Canvas`, `radiusSmall` corners (drawn as a rounded-square identicon — distinct from a circular avatar, reinforces "token/market" feel). Derivation from `maker_eth_address`:
- Strip `0x`. Parse hex chars in pairs.
- **Palette:** pick 2 hues from the theme accent family + status colors, keyed off `parseInt(addr.slice(2,4),16)`: base color = HSL where `hue = (byte/255)*360`, but **clamped into the app's warm/teal band** by blending 60% toward `Theme.accent` so identicons feel on-brand rather than rainbow. Secondary = `Theme.surfaceLight`.
- **Pattern:** a 5×5 grid, **mirrored left↔right** (classic GitHub identicon symmetry). Cell filled if the corresponding bit of the address bytes is set (`parseInt(addr[i],16) % 2`). Filled = base color, empty = transparent over a `Theme.inputBackground` tile.
- Background tile `Theme.inputBackground` (`#1A1A1A`), 1px inner `Theme.border`.

**Expiry chip** (top-right). Text = `expiresIn(min(lez_timelock, eth_timelock))` (the binding, earliest expiry). Pill: `radiusSmall`, height 20, horizontal pad 8, `fontSmall`. Color ramps by urgency:
- `> 30m`: bg `transparent`, border `Theme.border`, text `Theme.textSecondary`.
- `5–30m`: bg `Theme.warning` @ 15% alpha, text `Theme.warning` (`#f9a826`).
- `< 5m`: bg `Theme.error` @ 15% alpha, text `Theme.error` (`#e94560`), plus a slow blink (opacity 1↔0.55, 900ms) to signal "act now."
- `"expired"`: card is collapsing out (§3), chip reads `Theme.textMuted`.

---

## 3. Live Behaviors

**Auto-refresh Timer.** `Timer { interval: 4000; repeat: true; running: swapBackend.messagingConnected }` → calls `swapBackend.fetchOffers()`. 4s cadence balances liveliness vs. relay-drain cost. Guard: skip the tick if `swapBackend.offersLoading` is still true (don't stack drains).

**Merge + dedup (client-side live feed).** Maintain a JS array `offers` as the model, keyed by a stable `offerKey = hashlock + ":" + maker_eth_address` (hashlock alone should be unique per offer; the pair guards collisions). On each `fetchOffers()` result:
- For each incoming offer: if `offerKey` unseen → **insert** (mark `_isNew = true`, stamp `_seenAt = Date.now()`); if seen → update fields in place (keep original `_seenAt` so it doesn't re-pulse).
- Never wholesale-replace the array (that would kill animations and re-pulse everything). Splice inserts at their sorted position.
- Re-sort by `timestamp_ms` desc after merge; reassign to the `Repeater.model` property.

**Pruning.** On every tick and on a 1s local `Timer`:
- Remove offers where `expiresIn(...) === "expired"` (past `min(timelock)`), but first trigger the **collapse-out** animation, then splice after 260ms.
- Remove stale offers not seen in the last **3 refresh cycles** (`Date.now() - lastConfirmed > 13000`) — the relay dropped them; they're gone from the live feed. Same collapse-out.

**Insert animation.** New delegate enters with scale + opacity: `scale 0.92→1.0`, `opacity 0→1` over 220ms `Easing.OutBack` (slight overshoot = tactile "pop"). Implemented via `Component.onCompleted` kicking a `NumberAnimation`, or a `states`/`transitions` pair on an `_appeared` property.

**Freshness pulse.** For offers where `Date.now() - timestamp_ms < 12000` (12s), show:
- A **freshness dot** (top-left, 8px circle, `Theme.success` `#4ecca3`) with a `SequentialAnimation` on a surrounding glow ring: ring `opacity 0.6→0`, `scale 1→2.2`, 1400ms loop `Easing.OutQuad` — a radar "ping."
- A subtle **card glow:** the card border animates `Theme.accent`↔`Theme.success` isn't needed; instead a soft outer highlight via a second `Rectangle` behind the card (`color: Theme.success`, `opacity` pulsing 0.12↔0.0, blurred feel through low alpha + `radiusLarge+2`).
- When age crosses 12s, `running` on these animations flips false and the dot fades out (`Behavior on opacity`).

**Age treatment (older cards).** No hard states, a gentle recede: cards older than 5 min drop card `opacity` to 0.82 and the hero number desaturates one step (`Theme.textPrimary`→`Theme.textSecondary`) — present but clearly "seasoned." Keeps the freshest offers visually dominant.

**Collapse-out (expiry/stale).** `opacity 1→0`, `scale 1→0.9`, `Layout`-height/implicit collapse over 240ms `Easing.InCubic`, then splice from model. Because card heights are uniform and it's a `Flow`, remaining cards reflow smoothly.

**Hover lift.** `MouseArea hoverEnabled`. On enter: card `y`-nudge `-2`, border → `Theme.accent`, a drop of extra shadow via a background `Rectangle` at `Theme.background` darker offset. `Behavior on` the border color + y, 120ms.

---

## 4. Color & Typography Mapping (exact tokens)

| Element | Token | Value |
|---|---|---|
| App background | `Theme.background` | `#171717` |
| Card surface | `Theme.surface` | `#1E1E1E` |
| Button default / identicon tile edge / hover fill base | `Theme.surfaceLight` | `#2A2A2A` |
| Hero LEZ number | `Theme.textPrimary` | `#F0F0F0` @ 28px bold |
| "LEZ" unit suffix / "for X ETH" | `Theme.textSecondary` | `#999999` @ 18 / 15 |
| "for" word, rate line, timeAgo | `Theme.textMuted` | `#666666` @ 13 (mono) |
| Take offer (hover) fill + hover text-on-accent | `Theme.accent` / `#171717` | `#FF8800` |
| Button hover-in tween target | `Theme.accentHover` | `#FF9922` |
| Freshness dot + glow | `Theme.success` | `#4ecca3` |
| Expiry chip 5–30m | `Theme.warning` | `#f9a826` |
| Expiry chip <5m / expired urgency | `Theme.error` | `#e94560` |
| Card border default / chip border | `Theme.border` | `#333333` |
| Identicon backing | `Theme.inputBackground` | `#1A1A1A` |

Type scale in use: hero **28** (one custom step above `fontTitle`), unit `fontLarge` 18, subline `fontNormal` 15, meta/rate `fontSmall` 13. Mono ("Menlo, Courier New") for addresses and the rate. Radii: cards `radiusLarge` 12, button/identicon `radiusNormal`/`radiusSmall`, chips `radiusSmall` 6. Spacing rhythm: `spacingLarge` 24 board padding, `spacingNormal` 16 gutters + card padding, `spacingSmall` 8 intra-row.

---

## 5. Empty & Connection States

Single centered `Column` occupying the grid region (mutually exclusive with the `Flow`), chosen by state:

**A. Connecting** — `swapBackend.messagingLoading || swapBackend.messagingRetrying || !swapBackend.messagingConnected`:
- Pulsing `Theme.accent` ring (indeterminate), title **"Tuning into the market…"** (`fontTitle` `textPrimary`), subtitle = `swapBackend.messagingConnectionStatus` (`fontNormal` `textSecondary`). If `messagingRetrying`, subtitle prefixes "Reconnecting — ".

**B. Connected, zero offers** — connected but `offers.length === 0`:
- Big muted glyph (a drawn empty-tile identicon at 20% opacity), title **"The market's waking up"**, subtitle **"No live offers yet. Be the first to make one."**
- **Maker CTA button** (`Theme.accent`, `inputHeight`, `radiusNormal`): **"Make an offer"** → navigates to the maker/config flow (emit a signal the shell routes; config lives on another tab). This is the only place a maker CTA appears prominently.

**C. Disconnected/error** — connected false and not loading, or `!swapBackend.ready`:
- `Theme.error` dot, title **"Disconnected from relays"**, subtitle `messagingConnectionStatus`, and a **"Retry"** button (`Theme.surfaceLight`→`accent` hover) calling a reconnect (or `fetchOffers()` as a nudge).

Transitions between empty-state and grid cross-fade 180ms so the board never hard-cuts.

---

## 6. Status Strip (fixed, 48px, under header)

`Rectangle color: Theme.surface`, bottom `border Theme.border`. Three segments in a RowLayout, `spacingLarge`:

- **Balances (left):** `◈ ETH {weiToEth(swapBackend.ethBalance)} · LEZ {swapBackend.lezBalance}`. Labels `textMuted`, numbers mono `textPrimary`. If a balance is 0/empty, show `—` in `textMuted`.
- **Config readiness (center):** derived from `swapBackend.validationErrorsJson` (parse; empty array/`{}` = ready). Ready → `● Config ready` in `Theme.success`. Not ready → `▲ Config needed` in `Theme.warning`, clickable → routes to Config tab. This gates whether "Take offer" is enabled (see §7).
- **Live/refresh (right):** connection pill mirroring header — `● Live · {messagingPeerCount} peers` (`success`) / `◐ Connecting` (`warning`) / `● Offline` (`error`) — plus `⟳ {refreshedAgo}` (`textMuted`), where `refreshedAgo` is `timeAgo` of last successful `fetchOffers`. A tiny spinner shows while `offersLoading`.

The header's right-side chip is a compact duplicate of the live pill so status is glanceable even mid-scroll (though the strip is fixed anyway).

---

## 7. Accept Interaction

Deliberate, two-step, no accidental spends:

1. **Tap "Take offer".** The card flips to an **inline confirm overlay** (a `Rectangle` covering the card, `Theme.surface` @ 97%, `radiusLarge`, fades/slides in 160ms). Card does **not** navigate yet — confirmation stays in-context so the user keeps the surrounding market visible.
2. **Overlay contents:** compact summary — "You send **{weiToEth} ETH**, receive **{lez_amount} LEZ**", rate, `min` expiry countdown live-ticking, maker short-addr + identicon. Two buttons: **"Confirm swap"** (`Theme.accent`) and **"Cancel"** (text-only `textSecondary`).
   - **Guard:** if config not ready (`validationErrorsJson` non-empty) or `!swapBackend.ready`, the Confirm button is disabled (`opacity 0.5`) with a line "Finish setup in Config to trade" linking to the Config tab.
   - **Guard:** if the offer expires while the overlay is open (`expiresIn === "expired"`), swap Confirm for a disabled "Offer expired" and auto-dismiss the card via collapse-out after 1.2s.
3. **Confirm** → `swapBackend.acceptOfferAndStartTaker(offerObj)`. Immediately set a local `_accepting` flag → button shows a spinner + "Starting…", disable the rest of the card.
4. **Hand-off:** emit `signal takerStarted(var offer)` from the board root; the app shell switches to the **Taker progress tab**. Optimistically collapse-out the accepted card from the board (it's no longer a takeable live offer). If `acceptOfferAndStartTaker` surfaces an error via `validationErrorsJson`/a signal, restore the card and show an inline `Theme.error` toast on it.

Cancel → overlay fades out, card returns to normal.

---

## 8. QML Sketch (core structure)

```qml
import QtQuick 2.15
import QtQuick.Layouts 1.15
import QtQuick.Controls 2.15
import SwapTheme 1.0  // Theme singleton

Item {
    id: board
    anchors.fill: parent

    // ---- live model ----
    property var offers: []                 // JS array, keyed by hashlock:maker
    property double lastRefresh: 0
    signal takerStarted(var offer)          // shell routes to Taker tab

    function offerKey(o) { return o.hashlock + ":" + o.maker_eth_address }
    function rateOf(o) {
        var eth = parseFloat(swapBackend.weiToEth(o.eth_amount))
        return eth > 0 ? Math.round(parseFloat(o.lez_amount) / eth) : 0
    }
    function shortAddr(a) { return a.slice(0,6) + "…" + a.slice(-4) }

    function mergeOffers(incoming) {
        var byKey = {}
        for (var i = 0; i < offers.length; i++) byKey[offerKey(offers[i])] = offers[i]
        var now = Date.now()
        for (var j = 0; j < incoming.length; j++) {
            var o = incoming[j], k = offerKey(o)
            if (byKey[k]) { o._seenAt = byKey[k]._seenAt; o._isNew = false }
            else { o._seenAt = now; o._isNew = true }
            o._lastConfirmed = now
            byKey[k] = o
        }
        var merged = []
        for (var key in byKey) {
            var e = byKey[key]
            if (swapBackend.expiresIn(Math.min(e.lez_timelock, e.eth_timelock)) === "expired") continue
            if (now - e._lastConfirmed > 13000) continue          // stale prune
            merged.push(e)
        }
        merged.sort(function(a,b){ return b.timestamp_ms - a.timestamp_ms })
        offers = merged
        lastRefresh = now
    }

    Connections {
        target: swapBackend
        function onOffersReady(json) { board.mergeOffers(JSON.parse(json)) } // adapt to real signal
    }

    Timer { interval: 4000; repeat: true; running: swapBackend.messagingConnected
            onTriggered: if (!swapBackend.offersLoading) swapBackend.fetchOffers() }
    Timer { interval: 1000; repeat: true; running: true                       // local age/prune tick
            onTriggered: board.offers = board.offers.slice() }                 // force re-eval bindings

    Rectangle { anchors.fill: parent; color: Theme.background }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // ---------- HEADER (fixed) ----------
        Rectangle {
            Layout.fillWidth: true; Layout.preferredHeight: 72
            color: Theme.background
            RowLayout {
                anchors { fill: parent; leftMargin: Theme.spacingLarge; rightMargin: Theme.spacingLarge }
                ColumnLayout {
                    spacing: 2
                    Text { text: "LEZ ⇄ ETH MARKET"; color: Theme.textPrimary
                           font.pixelSize: Theme.fontTitle; font.bold: true }
                    Text { text: "Atomic swaps, settling in real time"
                           color: Theme.textSecondary; font.pixelSize: Theme.fontSmall }
                }
                Item { Layout.fillWidth: true }
                LivePill {}   // ● Live · N peers  (small reusable component)
            }
        }

        // ---------- STATUS STRIP (fixed) ----------
        Rectangle {
            Layout.fillWidth: true; Layout.preferredHeight: 48
            color: Theme.surface
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Theme.border }
            RowLayout {
                anchors { fill: parent; leftMargin: Theme.spacingLarge; rightMargin: Theme.spacingLarge }
                spacing: Theme.spacingLarge
                Text { font.pixelSize: Theme.fontSmall; textFormat: Text.StyledText
                       color: Theme.textSecondary
                       text: "◈ ETH <font color='#F0F0F0'>" + swapBackend.weiToEth(swapBackend.ethBalance)
                             + "</font>  ·  LEZ <font color='#F0F0F0'>" + swapBackend.lezBalance + "</font>" }
                Item { Layout.fillWidth: true }
                Text {
                    property bool ready: swapBackend.validationErrorsJson === "" || swapBackend.validationErrorsJson === "[]"
                    text: ready ? "● Config ready" : "▲ Config needed"
                    color: ready ? Theme.success : Theme.warning
                    font.pixelSize: Theme.fontSmall
                }
                Item { Layout.fillWidth: true }
                Text { text: "⟳ " + (board.lastRefresh ? swapBackend.timeAgo(board.lastRefresh) : "—")
                       color: Theme.textMuted; font.pixelSize: Theme.fontSmall }
            }
        }

        // ---------- BODY: grid OR empty state ----------
        Item {
            Layout.fillWidth: true; Layout.fillHeight: true

            // EMPTY / CONNECTING STATE
            ColumnLayout {
                anchors.centerIn: parent; spacing: Theme.spacingNormal
                visible: !swapBackend.messagingConnected || board.offers.length === 0
                Text {
                    horizontalAlignment: Text.AlignHCenter
                    color: Theme.textPrimary; font.pixelSize: Theme.fontTitle
                    text: !swapBackend.messagingConnected ? "Tuning into the market…" : "The market's waking up"
                }
                Text {
                    horizontalAlignment: Text.AlignHCenter; Layout.alignment: Qt.AlignHCenter
                    color: Theme.textSecondary; font.pixelSize: Theme.fontNormal
                    text: !swapBackend.messagingConnected ? swapBackend.messagingConnectionStatus
                                                          : "No live offers yet. Be the first to make one."
                }
                MarketButton {           // reusable accent button
                    Layout.alignment: Qt.AlignHCenter
                    visible: swapBackend.messagingConnected
                    label: "Make an offer"; onClicked: board.parent.gotoConfig()   // shell hook
                }
            }

            // CARD GRID
            ScrollView {
                anchors.fill: parent
                clip: true
                visible: swapBackend.messagingConnected && board.offers.length > 0
                Flow {
                    width: board.width
                    padding: Theme.spacingLarge
                    spacing: Theme.spacingNormal

                    Repeater {
                        model: board.offers
                        delegate: Rectangle {
                            id: card
                            width: 300; height: 248
                            radius: Theme.radiusLarge
                            color: Theme.surface
                            border.width: 1
                            border.color: hov.containsMouse ? Theme.accent : Theme.border
                            opacity: 0
                            scale: 0.92
                            property bool fresh: (Date.now() - modelData.timestamp_ms) < 12000
                            property string expLabel: swapBackend.expiresIn(Math.min(modelData.lez_timelock, modelData.eth_timelock))

                            Behavior on border.color { ColorAnimation { duration: 120 } }
                            Component.onCompleted: appear.start()
                            ParallelAnimation {
                                id: appear
                                NumberAnimation { target: card; property: "opacity"; to: 1; duration: 220 }
                                NumberAnimation { target: card; property: "scale"; to: 1; duration: 220; easing.type: Easing.OutBack }
                            }

                            MouseArea { id: hov; anchors.fill: parent; hoverEnabled: true }

                            ColumnLayout {
                                anchors { fill: parent; margins: Theme.spacingNormal }
                                spacing: Theme.spacingSmall

                                // top meta row: freshness dot + expiry chip
                                RowLayout {
                                    Layout.fillWidth: true
                                    Item {
                                        width: 10; height: 10; visible: card.fresh
                                        Rectangle { anchors.centerIn: parent; width: 8; height: 8; radius: 4; color: Theme.success }
                                        Rectangle {                     // radar ping
                                            id: ping; anchors.centerIn: parent; width: 8; height: 8; radius: width/2
                                            color: "transparent"; border.width: 2; border.color: Theme.success
                                            SequentialAnimation on scale { running: card.fresh; loops: Animation.Infinite
                                                NumberAnimation { from: 1; to: 2.4; duration: 1400; easing.type: Easing.OutQuad } }
                                            SequentialAnimation on opacity { running: card.fresh; loops: Animation.Infinite
                                                NumberAnimation { from: 0.6; to: 0; duration: 1400 } }
                                        }
                                    }
                                    Item { Layout.fillWidth: true }
                                    Rectangle {                          // expiry chip
                                        implicitHeight: 20; implicitWidth: chipTxt.width + 16; radius: Theme.radiusSmall
                                        property int urgency: card.expLabel === "expired" ? 3
                                                              : card.expLabel.indexOf("h") >= 0 ? 0 : 1  // simplistic; refine w/ seconds
                                        color: urgency === 1 ? Qt.rgba(0.976,0.658,0.149,0.15) : "transparent"
                                        border.width: 1; border.color: urgency === 1 ? "transparent" : Theme.border
                                        Text { id: chipTxt; anchors.centerIn: parent; text: card.expLabel
                                               font.pixelSize: Theme.fontSmall
                                               color: card.expLabel === "expired" ? Theme.textMuted
                                                     : urgency === 1 ? Theme.warning : Theme.textSecondary }
                                    }
                                }

                                // hero
                                RowLayout {
                                    Layout.topMargin: Theme.spacingSmall; spacing: 6
                                    Text { text: modelData.lez_amount; color: Theme.textPrimary
                                           font.pixelSize: 28; font.bold: true }
                                    Text { text: "LEZ"; color: Theme.textSecondary
                                           font.pixelSize: Theme.fontLarge; Layout.alignment: Qt.AlignBottom
                                           Layout.bottomMargin: 3 }
                                }
                                Text { text: "for " + swapBackend.weiToEth(modelData.eth_amount) + " ETH"
                                       color: Theme.textSecondary; font.pixelSize: Theme.fontNormal }
                                Text { text: "≈ " + board.rateOf(modelData) + " LEZ/ETH"
                                       color: Theme.textMuted; font.pixelSize: Theme.fontSmall
                                       font.family: "Menlo, Courier New" }

                                Item { Layout.fillHeight: true }

                                // maker row
                                RowLayout {
                                    Layout.fillWidth: true; spacing: Theme.spacingSmall
                                    Identicon { addr: modelData.maker_eth_address; size: 36 }  // Canvas component below
                                    ColumnLayout {
                                        spacing: 0
                                        Text { text: board.shortAddr(modelData.maker_eth_address)
                                               color: Theme.textPrimary; font.pixelSize: Theme.fontNormal
                                               font.family: "Menlo, Courier New" }
                                        Text { text: swapBackend.timeAgo(modelData.timestamp_ms)
                                               color: Theme.textMuted; font.pixelSize: Theme.fontSmall }
                                    }
                                }

                                // accept
                                MarketButton {
                                    Layout.fillWidth: true; Layout.preferredHeight: Theme.inputHeight
                                    label: "Take offer"
                                    onClicked: confirm.open = true
                                }
                            }

                            // inline confirm overlay
                            Rectangle {
                                id: confirm
                                property bool open: false
                                anchors.fill: parent; radius: Theme.radiusLarge
                                color: Qt.rgba(0.117,0.117,0.117,0.97)
                                visible: opacity > 0; opacity: open ? 1 : 0
                                Behavior on opacity { NumberAnimation { duration: 160 } }
                                ColumnLayout {
                                    anchors { fill: parent; margins: Theme.spacingNormal }
                                    spacing: Theme.spacingSmall
                                    Text { text: "Confirm swap"; color: Theme.textPrimary
                                           font.pixelSize: Theme.fontLarge; font.bold: true }
                                    Text { text: "You send " + swapBackend.weiToEth(modelData.eth_amount)
                                                 + " ETH\nYou receive " + modelData.lez_amount + " LEZ"
                                           color: Theme.textSecondary; font.pixelSize: Theme.fontNormal }
                                    Item { Layout.fillHeight: true }
                                    MarketButton {
                                        Layout.fillWidth: true; label: "Confirm swap"
                                        enabled: swapBackend.ready && card.expLabel !== "expired"
                                        onClicked: {
                                            swapBackend.acceptOfferAndStartTaker(modelData)
                                            board.takerStarted(modelData)
                                            confirm.open = false
                                        }
                                    }
                                    Text { text: "Cancel"; color: Theme.textSecondary
                                           font.pixelSize: Theme.fontNormal
                                           MouseArea { anchors.fill: parent; onClicked: confirm.open = false } }
                                }
                            }
                        } // card
                    } // Repeater
                } // Flow
            } // ScrollView
        } // body Item
    } // ColumnLayout
}
```

**Identicon component** (`Identicon.qml`, referenced above):

```qml
import QtQuick 2.15
import SwapTheme 1.0
Canvas {
    id: ic
    property string addr: ""
    property int size: 36
    width: size; height: size
    onAddrChanged: requestPaint()
    onPaint: {
        var ctx = getContext("2d"); ctx.reset()
        var hex = addr.replace("0x","")
        var byte = parseInt(hex.substr(0,2),16) || 0
        var hue = (byte/255)*360
        // blend 60% toward accent (#FF8800 ≈ hue 32) to stay on-brand
        hue = hue*0.4 + 32*0.6
        var base = Qt.hsla(hue/360, 0.7, 0.55, 1)
        ctx.fillStyle = Theme.inputBackground; ctx.fillRect(0,0,size,size)
        var cell = size/5
        for (var x=0; x<3; x++) {           // mirror cols 0-2 → 3-4
            for (var y=0; y<5; y++) {
                var nib = parseInt(hex[(x*5+y) % hex.length],16) || 0
                if (nib % 2 === 0) {
                    ctx.fillStyle = base
                    ctx.fillRect(x*cell, y*cell, cell, cell)
                    ctx.fillRect((4-x)*cell, y*cell, cell, cell)   // mirror
                }
            }
        }
    }
}
```

**Reusable `MarketButton.qml`** (referenced): background `Rectangle` `Theme.surfaceLight` → `Theme.accent` on hover (via `MouseArea hoverEnabled`, `Behavior on color`), `contentItem: Text` centered, `Theme.textPrimary` → `#171717` on hover, `radius: Theme.radiusNormal`, disabled → `opacity 0.5`.

---

### Build notes / decisions locked
- **Hero = LEZ amount**, rate is the secondary scan anchor — not the hero.
- **Explicit "Take offer" + inline confirm overlay**, never whole-card auto-accept.
- **4s refresh Timer**, client-side merge/dedup by `hashlock:maker`, 3-cycle stale prune + timelock expiry collapse.
- **Freshness < 12s** → success-green radar ping + soft glow; **> 5min** → gentle recede.
- Uniform 300×248 cards in a `Flow` for jank-free insert/collapse reflow.
- The `onOffersReady` signal name in `Connections` is a placeholder — wire it to however `fetchOffers()` surfaces results (return value polled after `offersLoading` flips false, or a real signal). The urgency ramp on the expiry chip should be refined to compute against raw `min(timelock) - now` seconds rather than string-sniffing "h"/"m".
