# Running the standing liquidity bot (`swap-cli maker --loop`)

The liquidity bot is the auto-accept maker loop packaged as an unattended
daemon: it advertises an offer over Waku, waits for takers to lock ETH on
Sepolia, locks LEZ, and claims the ETH once the taker reveals the preimage.
One process, coordination purely on-chain; the Waku heartbeat is a
best-effort advertisement sidecar.

```sh
swap-cli --env-file maker.env maker --loop
```

## What `--loop` adds over the plain maker

| Piece | Behaviour |
|---|---|
| Startup timelock guard | Refuses to start unless `ETH_TIMELOCK_MINUTES >= LEZ_TIMELOCK_MINUTES + margin` (margin `--timelock-margin-minutes`, default 5, min 5 — EthHTLC enforces `minTimelockDelta = 300s`). Taker locks first with the long timelock; maker locks second with the short one. **In `--loop` these default to `LEZ_TIMELOCK_MINUTES=20` / `ETH_TIMELOCK_MINUTES=40` when unset** — longer than the single-shot maker's 5/10, because LEZ lock *confirmation alone* can take up to 300s on the public testnet and a 5-minute LEZ window would leave the maker almost no margin. An explicit env/flag value overrides the default. |
| Startup inventory guard | Refuses to start if LEZ balance < `LEZ_AMOUNT`. |
| Heartbeat offer republish | Spawns `node offer-publisher/publish-offer.mjs` (override: `--publisher-script` / `OFFER_PUBLISHER_SCRIPT`), which republishes the offer every `--heartbeat-secs` (default 45, env `OFFER_HEARTBEAT_SECS`) with fresh timelocks. Needed because the fleet runs `store=false` — late-joining subscribers only see live messages. Supervised: restarted with 30s backoff if it dies. This is a **headless Node.js daemon dependency of `--loop`** (not browser tech): requires `node` >= 20 + `npm install` in `offer-publisher/`. See [Offer-publisher sidecar](#offer-publisher-sidecar-node-dependency) below. |
| Crash recovery | In-flight swaps are journaled to `--state-file` (default `.maker-state.json`, env `MAKER_STATE_FILE`). On startup each journaled escrow is checked on-chain: expired → LEZ refunded (feeless); taker already claimed → ETH claimed with the revealed preimage (profit recovered); still live → resumed in a background watcher; terminal → dropped. |
| Faucet sidecar | `--fund-to <target>` (env `FUND_TO_TARGET`) loops `wallet pinata claim --to <maker>` (150 LEZ per claim, feeless, repeatable) until the balance reaches the target. Standalone (`maker --fund-to 3000` then exit) or combined with `--loop` (tops up before the loop starts). Wallet binary path: `--wallet-bin` / `LEZ_WALLET_BIN`. Requires wallet-mode auth (`LEZ_WALLET_HOME` + `LEZ_ACCOUNT_ID`). |
| Graceful stop | Ctrl-C / SIGINT stops after the current wait; out-of-inventory stops the loop cleanly. |

## Offer-publisher sidecar (Node dependency)

The `--loop` heartbeat is served by a **headless Node.js daemon**,
`offer-publisher/publish-offer.mjs`, that `swap-cli` spawns and supervises. It
is **not** a website and has no browser/UI code — it connects once to the
logos.dev delivery fleet over raw TCP (`@waku/sdk` + `@libp2p/tcp`) and
lightpushes the offer JSON every heartbeat.

**Why Node and not pure Rust?** The fleet runs `store=false`, so the offer has
to be re-broadcast on an interval or late-joining subscribers never see it. The
Rust `swap-cli` links **no delivery/Waku client** (it coordinates every swap
purely on-chain), so the only publish path that exists today is this JS
sidecar — it emits both the initial offer and every republish. A pure-Rust
republish is the intended follow-up (it would let us drop Node entirely), but it
is a large change: it requires introducing a Rust logos-delivery/Waku lightpush
client wire-compatible with cluster-2 autosharding and the offer schema, none of
which is in the dependency tree yet. Until then, **Node >= 20 is a hard runtime
dependency of the heartbeat only** — swaps still complete without it (they
coordinate on-chain); only the offer advertisement stops.

Setup (once, in the repo checkout):

```sh
cd offer-publisher && npm install
```

> **The browser offer board is not in this repo.** The viewer UI that displays
> these offers now lives in the `swap_ui` Basecamp app (home screen). This
> sidecar only *publishes* offers; it never renders them.

## Configuration

Everything the plain CLI takes (`.env` / flags — see `docs/testnet.md`), plus
the flags above. A public-testnet `maker.env` looks like:

```ini
ETH_RPC_URL=wss://...sepolia...        # WebSocket required
ETH_PRIVATE_KEY=0x...
ETH_HTLC_ADDRESS=0x8636Fe66...834A
ETH_RECIPIENT_ADDRESS=0x...            # maker address receiving ETH
LEZ_SEQUENCER_URL=https://testnet.lez.logos.co
LEZ_WALLET_HOME=.scaffold/wallet
LEZ_ACCOUNT_ID=<maker base58>
LEZ_HTLC_PROGRAM_ID=<64 hex>
LEZ_TAKER_ACCOUNT_ID=<designated taker's LEZ account, base58>   # the ONE counterparty
LEZ_AMOUNT=150
ETH_AMOUNT=0.001
LEZ_TIMELOCK_MINUTES=20                 # --loop default when unset (single-shot: 5)
ETH_TIMELOCK_MINUTES=40                 # --loop default when unset (single-shot: 10)
OFFER_HEARTBEAT_SECS=45
MAKER_STATE_FILE=/var/lib/lez-maker/state.json
RESTRICT_COUNTERPARTY=true              # required to start --loop; accepts 1/0/true/false/yes/no (see below)
```

### Designated-taker requirement (read before you set `LEZ_TAKER_ACCOUNT_ID`)

The LEZ HTLC `Claim` instruction is gated on `signer == taker_id`: only the
account you name in `LEZ_TAKER_ACCOUNT_ID` can ever claim the LEZ the maker
locks. The loop has **no inbound channel** to learn a public taker's LEZ
account per-swap (offer advertisement is publish-only), so every escrow it creates
is locked to that one static account. `--loop` therefore refuses to start
unless you pass `--restrict-counterparty` (`RESTRICT_COUNTERPARTY=true`) to
acknowledge this — the standing bot serves a **single, pre-arranged
counterparty**, not the open public.

Real designated-taker flow:

1. Agree the swap with your counterparty out-of-band.
2. Obtain their LEZ account id (base58): `wallet account list` on their side,
   or have them send it to you.
3. Set `LEZ_TAKER_ACCOUNT_ID=<their base58 account>` in `maker.env`.
4. Start with `--restrict-counterparty` (or `RESTRICT_COUNTERPARTY=true`).

> **⚠️ WARNING — funds lock to whatever taker you configure.** The maker
> transfers `LEZ_AMOUNT` into an escrow that **only** `LEZ_TAKER_ACCOUNT_ID`
> can claim. If that value is a placeholder, a typo, or the wrong account, the
> LEZ is **not lost but is stranded**: no one can claim it, and the maker can
> only recover it by **refunding after `LEZ_TIMELOCK_MINUTES` expires**
> (default 20 min per swap). Never point the bot at a placeholder account —
> set the real counterparty before you start.

## Funding lifecycle

The maker **sells LEZ for ETH**; economics are asymmetric:

- **LEZ drains** — each completed swap ships `LEZ_AMOUNT` to the taker.
  Refills: pinata faucet, 150 LEZ/claim, feeless and repeatable.
  `claims/day = swaps/day x LEZ_AMOUNT / 150`. At `LEZ_AMOUNT=1000`,
  10 swaps/day = ~67 claims/day; at `LEZ_AMOUNT=150` one claim funds one
  swap. Run a cron/timer with `swap-cli maker --fund-to <N x LEZ_AMOUNT>`.
- **Sepolia ETH barely drains** — the maker's *only* gas cost is the
  profitable `claim` (~50-60k gas). At 1 gwei that is ~5.5e-5 ETH/claim
  (~1000 swaps per 0.059 ETH); at 20 gwei ~53 swaps. Treat gas as a
  slow-draining reserve with a low-water alarm; **LEZ inventory, not gas,
  gates throughput.**
- LEZ lock / fund / refund are all feeless.

## systemd

```ini
# /etc/systemd/system/lez-maker.service
[Unit]
Description=LEZ liquidity maker
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=maker
WorkingDirectory=/opt/eth-lez-atomic-swaps
EnvironmentFile=/opt/lez-maker/maker.env
ExecStart=/opt/lez-maker/swap-cli --env-file /opt/lez-maker/maker.env maker --loop
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```ini
# /etc/systemd/system/lez-maker-fund.timer (+ matching .service) — hourly top-up
[Timer]
OnCalendar=hourly
# service: ExecStart=/opt/lez-maker/swap-cli --env-file /opt/lez-maker/maker.env maker --fund-to 1500
```

`WorkingDirectory` should be the repo checkout (or set
`OFFER_PUBLISHER_SCRIPT` to an absolute path) so the heartbeat sidecar is
found; run `npm install` in `offer-publisher/` once.

## launchd (macOS)

```xml
<!-- ~/Library/LaunchAgents/co.logos.lez-maker.plist -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>co.logos.lez-maker</string>
  <key>ProgramArguments</key><array>
    <string>/opt/lez-maker/swap-cli</string>
    <string>--env-file</string><string>/opt/lez-maker/maker.env</string>
    <string>maker</string><string>--loop</string>
  </array>
  <key>WorkingDirectory</key><string>/opt/eth-lez-atomic-swaps</string>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/tmp/lez-maker.log</string>
  <key>StandardErrorPath</key><string>/tmp/lez-maker.log</string>
</dict></plist>
```

## Griefing analysis (summary)

Taker-locks-first makes griefing costly to the attacker and ~free to the maker:

- Taker abandons after maker locks LEZ → LEZ (short) timelock expires first,
  maker refunds **feelessly**; the griefer must later pay Sepolia gas to
  refund their own ETH. Maker loses only the LEZ capital lock-up window
  (`LEZ_TIMELOCK_MINUTES`, default 20).
- Fake accepts cost nothing: the maker only locks LEZ **after** verifying a
  matching, still-`OPEN` ETH lock on-chain.
- Residual DoS: capital lock-up via many parallel accepts. Mitigations: keep
  `LEZ_TIMELOCK_MINUTES` tight; keep per-swap inventory small
  (`LEZ_AMOUNT=150`); the loop is sequential (one in-flight swap at a time),
  which itself caps exposure to one escrow.

## Monitoring hints

- **Progress stream**: run with `--json` for one JSON object per event
  (`step`/`data`), ideal for shipping to a log pipeline. Alert on
  `AutoAcceptInsufficientFunds` (loop stopped — refill and restart) and on
  repeated `AutoAcceptSwapFailed`.
- **Heartbeat**: `[offer-publisher]` lines show publish acks; the supervisor
  logs restarts. If the offer stops being advertised, check `node` is
  installed and the fleet is reachable (`npm run smoke` in `offer-publisher`).
- **Balances**: watch maker LEZ balance vs `LEZ_AMOUNT` (loop stops below it)
  and Sepolia ETH vs ~1e-4 ETH per claim.
- **State file**: a non-empty `.maker-state.json` after a crash is normal —
  reconciliation clears it on the next start. Alert if entries persist
  across restarts (RPC/sequencer trouble).

## Quarantined swaps (partial-lock wedges)

The LEZ lock is two transactions: the `Lock` instruction (creates the escrow
PDA in state `Locked`) and a funding transfer. If the process dies between the
two — or the sequencer commits the lock but rejects the transfer — the escrow
is left `Locked` **with zero balance**. That escrow can never be terminalized:
the HTLC program's `Refund` requires `balance >= amount`, so a refund can never
confirm, and a resume watcher would wait until expiry and then retry forever.
Left in the active journal, such an entry would wedge sequential startup into a
restart-forever loop.

Startup reconciliation therefore detects this state (an **expired** `Locked`
escrow whose balance is confirmed below the locked amount across several
re-reads — an unexpired underfunded escrow is given the benefit of the doubt
and resumed, since its funding transfer may still be in flight after a fast
restart) and **quarantines** the entry:

- It is moved from `in_flight` to a separate `quarantined` section of the
  state file, with a reason and timestamp.
- Quarantined entries **never block startup** and are **never retried** — the
  bot proceeds to accept new swaps.
- Every startup logs an `ERROR` line per quarantined entry so the condition
  stays visible.

**Operationally, a quarantined entry means:**

- **No LEZ is sitting in the escrow** in the common case (the funding never
  landed, so the PDA holds 0), but the *hashlock is burned*: the PDA for that
  hashlock exists forever in `Locked` state and can never be locked, claimed,
  or refunded.
- **The secret behind that hashlock is permanently unusable — never reuse
  it.** A new swap keyed by the same hashlock would collide with the dead PDA:
  the maker's lock refuses to fund it, and anything transferred to that PDA is
  unrecoverable. Takers generate fresh secrets per swap, so this only matters
  if someone replays an old secret by hand.
- The entries are kept purely for audit. After verifying the escrow balance is
  0 (`swap-cli status --hashlock <hex>`), you may prune the `quarantined`
  array from the state file for a clean file; keeping them costs nothing.

Alert on the startup warning (`quarantined partial-lock hashlock(s)`): each
occurrence indicates a crash inside the lock sequence or a sequencer that
accepted a lock but dropped its funding — worth investigating even though the
bot keeps running.
