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
| Startup timelock guard | Refuses to start unless `ETH_TIMELOCK_MINUTES >= LEZ_TIMELOCK_MINUTES + margin` (margin `--timelock-margin-minutes`, default 5, min 5 — EthHTLC enforces `minTimelockDelta = 300s`). Taker locks first with the long timelock; maker locks second with the short one. |
| Startup inventory guard | Refuses to start if LEZ balance < `LEZ_AMOUNT`. |
| Heartbeat offer republish | Spawns `node web/offer-board/publish-offer.mjs` (override: `--publisher-script` / `OFFER_PUBLISHER_SCRIPT`), which republishes the offer every `--heartbeat-secs` (default 45, env `OFFER_HEARTBEAT_SECS`) with fresh timelocks. Needed because the fleet runs `store=false` — late-joining board viewers only see live messages. Supervised: restarted with 30s backoff if it dies. Requires `node` + `npm install` in `web/offer-board/`. |
| Crash recovery | In-flight swaps are journaled to `--state-file` (default `.maker-state.json`, env `MAKER_STATE_FILE`). On startup each journaled escrow is checked on-chain: expired → LEZ refunded (feeless); taker already claimed → ETH claimed with the revealed preimage (profit recovered); still live → resumed in a background watcher; terminal → dropped. |
| Faucet sidecar | `--fund-to <target>` (env `FUND_TO_TARGET`) loops `wallet pinata claim --to <maker>` (150 LEZ per claim, feeless, repeatable) until the balance reaches the target. Standalone (`maker --fund-to 3000` then exit) or combined with `--loop` (tops up before the loop starts). Wallet binary path: `--wallet-bin` / `LEZ_WALLET_BIN`. Requires wallet-mode auth (`LEZ_WALLET_HOME` + `LEZ_ACCOUNT_ID`). |
| Graceful stop | Ctrl-C / SIGINT stops after the current wait; out-of-inventory stops the loop cleanly. |

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
LEZ_TAKER_ACCOUNT_ID=<any placeholder — real taker comes from the accept>
LEZ_AMOUNT=150
ETH_AMOUNT=0.001
LEZ_TIMELOCK_MINUTES=20
ETH_TIMELOCK_MINUTES=40
OFFER_HEARTBEAT_SECS=45
MAKER_STATE_FILE=/var/lib/lez-maker/state.json
```

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
found; run `npm install` in `web/offer-board/` once.

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
  logs restarts. If the offer stops appearing on the board, check `node` is
  installed and the fleet is reachable (`npm run smoke` in `web/offer-board`).
- **Balances**: watch maker LEZ balance vs `LEZ_AMOUNT` (loop stops below it)
  and Sepolia ETH vs ~1e-4 ETH per claim.
- **State file**: a non-empty `.maker-state.json` after a crash is normal —
  reconciliation clears it on the next start. Alert if entries persist
  across restarts (RPC/sequencer trouble).
