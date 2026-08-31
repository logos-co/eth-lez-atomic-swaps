# In-house Sepolia ETH drip faucet — proof of concept

**Status: PoC for review. Not a merge candidate, not deployed anywhere, and
deliberately not run through the full validation pipeline.**

Built to the architecture the scout report recommends (option A): a small
custom Rust drip service gated by the same proof-of-work idiom this repo
already ships for the LEZ pinata faucet, plus a "Get test ETH" button in the
Setup step's ETH funding section.

---

## The problem, in one paragraph

`SetupView.qml`'s step 4 can tell a new user their address and watch for ETH to
arrive, but it cannot get them any: it hands over a copyable link to
`sepolia-faucet.pk910.de` and the user leaves the app to mine in a browser tab.
The app already funds LEZ automatically. This closes the other half — one
press, no browser, no account, no address copy-paste, because the app already
knows the address.

The reason it needs a server at all: the recipient has **0 ETH**, so somebody
else must pay the gas for the first transfer. There is no design where the user
claims on-chain themselves.

---

## What is here

| Piece | Path | What it is |
|---|---|---|
| Shared PoW scheme | `eth-faucet-pow/` | The hash rule + bounded solver. One crate, used by BOTH the service and the app, so verifier and solver cannot drift. |
| The service | `eth-faucet/` | axum + alloy. `/challenge`, `/drip`, `/health`, `/stats`. |
| App client | `swap-ffi/src/faucet_client.rs` | `swap_ffi_faucet_request_eth(url, address)` — challenge, solve, claim, in one blocking call. |
| Module glue | `swap-module/src/swap_impl.{h,cpp}` | `SwapImpl::faucetRequestEth` — the RPC surface the generator exposes to swap-ui. |
| UI | `swap-ui/src/qml/SetupView.qml`, `swap_ui_plugin.cpp` | The button, its pending/success/error states, and the URL config. |
| Deploy | `deploy/Dockerfile.faucet`, `deploy/docker-compose.faucet.yml`, `deploy/faucet.env.example` | One container, one gitignored env file, one named volume. |
| Demo | `make faucet-poc-*`, `scripts/faucet-poc-demo.sh` | Run it locally against real Sepolia with a key you generate. |

---

## The proof-of-work gate

Same shape as `src/lez/faucet.rs` (the pinata faucet): find a `u128` `solution`
such that `SHA256(seed || solution.to_le_bytes())` starts with enough zeros.

**One deliberate deviation: difficulty counts zero BITS, not zero BYTES.** The
pinata scheme's byte granularity multiplies the work by 256 per step — three
zero bytes is a few seconds and four is over an hour, with nothing in between.
A faucet wants to aim at a target solve time, and wants to be able to *raise*
difficulty as the day's budget depletes (pk910's trick), so it needs a knob it
can turn by 2x rather than by 256x.

| Difficulty | Expected hashes | Roughly |
|---|---|---|
| 18 bits | 262 144 | instant (the local demo default) |
| 24 bits | 16.8 million | a few seconds (the service default) |
| 26 bits | 67 million | ~15–30 s |
| 27 bits | 134 million | ~30–60 s |

The seed is drawn from the OS CSPRNG per request and stored server-side against
that one address, which is what makes a solution un-precomputable (you do not
know the seed until you ask), un-replayable (accepting it consumes the
challenge), and un-transplantable to another address (a different address got a
different seed). Re-asking **replaces** the outstanding challenge rather than
adding one, so nobody can bank a pile of pre-solved puzzles to spend the moment
a cooldown lapses.

## Drain resistance

Four independent limits, checked in that order so the refusal a user reads
names the rule that actually stopped them:

| Control | Default | What it is for |
|---|---|---|
| Per-address cooldown | 24 h | The main honest-use limit. |
| Per-address lifetime cap | 0.1 ETH | Survives cooldown expiry — waiting does not reset it. |
| Per-IP cooldown | 1 h | **Secondary signal only.** VPNs are free and app users share NATs, so it blunts a script loop and is never the gate. |
| Global daily budget | 1 ETH/UTC day | **The backstop.** No combination of addresses, IPs, or solved puzzles spends more than this in a day, whatever gets past the rest. |

Plus a "you already hold ≥ one drip" refusal, which is a UX courtesy rather
than a defense (a sybil just uses empty addresses) — and is checked *after* the
PoW so it cannot be used as a free balance oracle.

Worst case under continuous attack at the defaults: 1 ETH/day, 50 sybil
addresses each paying a CPU-second, against a reserve that `/health` reports on
in days-of-budget — so the runway is visible before it runs out.

**Custody, unchanged from the scout's recommendation:** the key here is hot on
an internet-facing host. Keep ≤ 5 ETH on it and the reserve cold. Phase 2 (a
`FaucetVault` contract enforcing the same caps on-chain, with the VPS key
demoted to a relayer) makes that split structural rather than a policy — see
"What this PoC deliberately does not do".

---

## HTTP surface

### `GET /challenge?address=0x…`

```json
{
  "address": "0x000000000000000000000000000000000000dead",
  "seed": "3f2a…",              // 32 bytes, hex
  "difficulty_bits": 24,
  "expires_at": 1756704000,
  "expected_hashes": 16777216,
  "drip_wei": "20000000000000000",
  "drip_eth": "0.02"
}
```

Issuing costs no allowance and checks no cooldown — deliberately. A
rate-limited user should learn that from `/drip` after a solve, not from an
endpoint that would otherwise let anyone read the ledger's contents about any
address for free.

### `POST /drip`

```json
{ "address": "0x…", "pow_solution": "1234567" }
```

`pow_solution` is a **string**: it is a `u128`, and JSON numbers are not
reliably that wide (JavaScript's are not). A solution that silently lost its
low bits in transit would look, to the user, like a faucet rejecting correct
answers.

On success — after the transaction has been **mined**, not merely submitted:

```json
{
  "tx_hash": "0x…", "address": "0x…",
  "amount_wei": "20000000000000000", "amount_eth": "0.02",
  "chain_id": 11155111
}
```

On refusal, `4xx`/`5xx` with a stable code and a sentence written for the
person in the Setup card:

```json
{ "error": { "code": "address_cooldown",
             "message": "This address already got test ETH recently. Try again in 23 hours." } }
```

Codes: `address_cooldown`, `ip_cooldown`, `lifetime_cap`, `daily_budget`,
`already_funded`, `no_challenge`, `challenge_expired`, `bad_solution`,
`bad_address`, `send_failed`.

### `GET /health`

`200` when the RPC answers and the balance covers at least one day's budget;
`503` otherwise, with `rpc_ok`, `balance_eth` and `days_of_budget_remaining`
saying which. This is what the container healthcheck runs — the same answer the
app and any uptime monitor get, so the two cannot drift.

### `GET /stats`

Balance, drips served, total dripped, today's spend against the budget, and
**every limit currently in force** — so "why did it refuse me" is answerable
without shell access to the VPS.

---

## Run it locally (real Sepolia, throwaway key)

Nothing here touches an existing key. `make faucet-poc-genkey` generates a new
one and refuses to overwrite an existing `.faucet-poc/faucet.env`, because
clobbering the key of a faucet you already funded strands that ETH.

```bash
# 1. Generate a throwaway key. Prints the address to fund.
make faucet-poc-genkey

# 2. Send it a little Sepolia ETH (any external faucet — this is the one
#    manual step, and the whole point is that users never have to do it).

# 3. Run the service in the foreground.
make faucet-poc-run

# 4. In another terminal — the full challenge -> solve -> drip round trip.
make faucet-poc-demo ADDRESS=0xYourEmptyTestAddress
```

`make faucet-poc-test` runs the unit tests (PoW, cooldowns, budget, journal,
URL resolution) with no network, no key and no chain.

### The same thing in raw curl

```bash
BASE=http://127.0.0.1:8787
ADDR=0xYourEmptyTestAddress

curl -sS $BASE/health

# Ask for a puzzle.
curl -sS --get $BASE/challenge --data-urlencode "address=$ADDR"
# -> {"seed":"3f2a…","difficulty_bits":18,…}

# Solve it with the same crate the app and the service use.
SOLUTION=$(cargo run --release -q -p eth-faucet -- --solve 3f2a… 18)

# Claim.
curl -sS -H 'Content-Type: application/json' \
     -d "{\"address\":\"$ADDR\",\"pow_solution\":\"$SOLUTION\"}" \
     $BASE/drip
# -> {"tx_hash":"0x…","amount_eth":"0.02","chain_id":11155111}

# Immediately again -> 429 address_cooldown. And:
curl -sS $BASE/stats
```

### The button in the app

`SWAP_UI_ETH_FAUCET_URL` points swap-ui at a faucet.

- **unset** → the compiled default, `http://127.0.0.1:8787` — so a reviewer
  running `make faucet-poc-run` gets a working button with no configuration.
- **set to a URL** → that faucet.
- **set and empty** → the button is hidden and step 4 is exactly what 0.4.6
  shipped. That is the off switch, and the reason "unset" and "set to empty"
  mean different things here.

The external faucet link stays in the card either way, re-framed as the
fallback ("Faucet busy, empty, or unreachable?"). One hot key on one VPS with a
daily budget will sometimes say no; when it does, the path that always worked
must still be on screen.

**Before any release the compiled default must become the deployed URL, or the
button must be hidden.** A shipped app whose faucet default is localhost is a
dead button with a confusing error — worse than the copy-link it sits beside.

---

## Deploying to the VPS

```bash
cd deploy
cp faucet.env.example faucet.env      # fill in a THROWAWAY key; gitignored
docker compose -f docker-compose.faucet.yml up -d --build
curl -fsS localhost:8787/health
```

A separate compose file from the makers on purpose: the two have nothing to do
with each other, they fail independently, and during a trial the faucet will be
restarted far more often than a maker holding an in-flight swap should be.

Notes that matter:

- **Put TLS in front of it.** The compose file binds `127.0.0.1:8787`, not
  `0.0.0.0`. The app must reach it over HTTPS through a reverse proxy that sets
  `X-Forwarded-For` — the per-IP limit reads that header, and a plaintext
  faucet hands every address a user funds to anything on the path.
- **The state volume is not optional.** `faucet-state:/app/state` holds the
  rate-limit journal. Losing it resets every cooldown, which turns the limits
  into suggestions until it refills. It is a named volume rather than a bind
  mount for exactly that reason.
- **The container is hardened** as far as a key-holding process cheaply can be:
  read-only root filesystem, non-root user, `no-new-privileges`, one writable
  path.
- **Refill on `/health`.** It goes 503 below one day's budget, which is one
  day's warning. `/stats` says how much today has spent.
- **Balances and receipts, never logs.** The pinned public Sepolia RPC has been
  observed in this project returning empty `eth_getLogs` results with no error
  about half the time. The faucet reads balances and waits on receipts and so
  is unaffected — worth keeping true if anyone extends its accounting.

---

## Tests

```
cargo test -p eth-faucet-pow -p eth-faucet     # 42 tests
make swap-ui-unit                              # includes eth_faucet_config_test
```

What they cover, and why those:

- **PoW** (`eth-faucet-pow`): a solve the verifier accepts; a solution for one
  seed rejected against another (the address binding); an easy solution
  rejected at a higher difficulty (or "raise difficulty under load" would be a
  no-op); the iteration/deadline/cancel bounds; the bit-counting across byte
  boundaries.
- **Challenges**: accepted exactly once then gone (replay); a wrong answer
  refused *without* burning the challenge; an expired one refused and cleared;
  claimable at the exact expiry instant; re-issuing replaces rather than
  accumulates.
- **Cooldowns and budget**: the boundary second of each cooldown; a fresh
  address not resetting the IP limit; an empty IP degrading to *no* limit
  rather than one shared bucket that would lock everyone out; the lifetime cap
  outliving the cooldown; a lowered cap refusing rather than wrapping; the
  budget stopping everyone, not just the spender; the UTC day roll needing no
  timer; a backwards clock not clearing a cooldown.
- **The reserve/rollback race**: two simultaneous requests for one address —
  only one passes; and a failed send restoring the ledger *exactly*, so a user
  is never charged a 24 h cooldown for ETH that never arrived.
- **Journal**: round trip through disk with the cooldown surviving; a corrupt
  journal refusing to read as empty (silently starting from zero would hand
  every rate-limited address a fresh allowance).
- **UI config**: unset vs. set-and-empty; trailing-slash and scheme handling.

Not covered, and honestly: there is no end-to-end test that stands up the
service and drives `/drip` against a chain. `make faucet-poc-demo` is that
test, run by hand.

---

## What this PoC deliberately does not do

- **No `FaucetVault` contract (phase 2).** The service signs value transfers
  from a hot key, so drain resistance is only as good as this process's own
  logic. The scout's phase 2 moves the reserve into a contract enforcing the
  same caps on-chain, demotes the VPS key to a relayer holding ~0.1 ETH of gas,
  gives the captain an offline `pause()`, and makes the contract address the
  published crowdfund deposit address. Everything here survives that change:
  the service switches from "sign a transfer" to "call `drip()`", and no client
  or UI code moves.
- **No difficulty ramp.** `FAUCET_POW_DIFFICULTY_BITS` is static. Raising it as
  the daily budget depletes is a few lines against `spent_today` and the
  obvious next tuning knob if abuse appears.
- **No crowdfund address published**, no docs updated beyond this file, and the
  dead `sepoliafaucet.com` link in `docs/community-install.md` is still dead —
  all of that belongs with the go-ahead decision, not with a PoC.
- **No release/CI wiring.** The service is a workspace member and a Dockerfile;
  it is not in `ci.yml`, has no GHCR image workflow, and the module changes
  have not been built through Nix here (`swap-module`/`swap-ui` need a real
  `nix build`, which is the check — see the Makefile's own note).
- **A blocking FFI call, not a job.** One press runs challenge→solve→claim as a
  single blocking call with one progress phase, rather than the streamed
  job the LEZ funding flow uses. Honest for a ~30 s operation with no
  meaningful intermediate states, and it keeps the module surface to one
  method. If drips ever get slow enough to need a stepper, that is the change.

## Review pointers

If you read three files, read these:

1. `eth-faucet/src/ledger.rs` — the limits, and the reserve/rollback ordering
   that makes concurrent claims and failed sends both behave.
2. `eth-faucet-pow/src/lib.rs` — the scheme, and the bits-not-bytes argument.
3. `swap-ui/src/qml/SetupView.qml`, step 4 — the button, and how its progress
   and errors stay inside its own card (`setupOrigin`, the lesson of #173).
