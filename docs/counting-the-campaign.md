# Counting the public trial (issue #98)

The owner's ask (issue #62 / #98): *"a CTA at the end — something that helps
me track how many ppl actually tried, and if they have feedback."*

PR #94 shipped the **feedback** half: a Basecamp-compatible public-trial form
(`.github/ISSUE_TEMPLATE/trial-feedback.yml`) with "Copy safe evidence" /
"Copy feedback link" end-state actions. This document covers the **counting**
half — the durable, queryable answer to "how many people actually tried
this, and how did it go?" — plus the runbook for both.

There is **no telemetry** anywhere in this project and this feature adds
none: every number below comes from either the public Ethereum chain, a file
the maker operator already owns, or a `gh` query against data the operator
(or a tester) put on GitHub voluntarily. Nothing here makes an outbound call
the app wouldn't otherwise make.

## Four numbers, four very different meanings

| # | Mechanism | What it counts | Kind of number |
|---|-----------|-----------------|-----------------|
| 1 | **On-chain report (`swap-cli chain-report`)** | Swaps whose **ETH leg was locked** at the pinned Sepolia venue — every attempt, whether or not a maker answered | **Exact, global, retroactive, publicly verifiable** |
| 2 | Ops ledger (`swap-cli maker --ops-report`) | Swaps *one* maker **accepted and served**, and the bounded `FailureCode` saying **why** a failure failed | Exact for that maker; the only source for *why* |
| 3 | `trial-feedback` GitHub issues | People who **chose to tell us** something | Exact count, but a self-selected sample |
| 4 | Release asset downloads | Times a `.lgx` module artifact was **fetched** | A ceiling only — includes bots/CDN/re-downloads |

None of these four is "number of distinct people who tried Atomic Swaps."
(1) counts swaps and wallets, not people, and deliberately does not attempt
to fingerprint a peer or IP to get closer to a person-count — see
[Privacy boundary](#privacy-boundary). Use (1) as the primary count of
attempts and outcomes; (2) to explain the failures (1) can only see as
"refunded" or "still open"; (3) as qualitative color; (4) only as an upper
bound on installs.

### Why the chain is the primary count, and the ops ledger is not

The ops ledger came first ([§2](#2-the-ops-ledger--why-a-swap-failed)) and is
still the right tool for the job it is uniquely good at. It is the wrong tool
for the headline number, for three reasons:

- **It only counts swaps a maker *accepted*.** A taker who locked ETH and got
  no counterparty at all — nobody on the offer board, or every maker out of
  inventory — never appears in any ledger, because no maker ever reserved
  anything for them. They locked real funds and had to wait out a timelock to
  get them back. During a public trial that is the single most important
  signal there is, and the ledger is structurally blind to it.
- **It is per-maker and per-container.** The fleet runs five makers; each has
  its own `maker-ops.jsonl` inside its own container, so a fleet-wide count
  needs operator access to all five and a manual sum. The chain has one venue
  and one answer.
- **It is not retroactive, and it is not verifiable by anyone else.** A
  ledger only contains what was recorded while it was running, so counting a
  trial period properly means snapshotting a baseline *before* the
  announcement goes out. The chain already holds the whole history: a trial
  window can be measured after the fact, and anyone — not just the operator —
  can reproduce the number from public data.

What the chain **cannot** do is say *why* a swap failed. `Refunded` and
"still open" are the only failure shapes it has; it cannot distinguish "the
maker's ETH claim never confirmed" from "the taker walked away". That is
exactly what the ops ledger's bounded `FailureCode` enum is for, and why it
stays.

### The attempted-vs-accepted gap

The two counts answer different questions and will not match. The chain's
`attempted` is always **≥** the sum of the ledgers' `accepted_total`, and the
difference is the interesting part:

```
chain attempted            takers who locked ETH at the venue
  − Σ ledger accepted      … of whom some maker reserved a swap
  ─────────────────────
  = takers no maker ever answered
```

A large gap means the market was not there when people showed up — makers
offline, out of LEZ inventory, or filtered out by a timelock/counterparty
guard — not that people were uninterested. A gap of zero means every taker
who locked found a counterparty. Reading only the ledger makes the first case
look identical to a quiet trial.

Two caveats on the subtraction: it needs the ledgers from *every* maker on
the board (see [§2](#2-the-ops-ledger--why-a-swap-failed)), and a swap
accepted just before the window closed can be counted in one and not the
other. Treat it as an indicator, not an accounting identity.

---

## 1. The on-chain count — every attempt, from the chain itself

Implementation: `src/eth/report.rs` (counting, log decoding, corroboration)
and `src/cli/report.rs` (the command). Read-only: `eth_getLogs` and
`eth_getBlockByNumber`, nothing else. No key, no gas, no state written, and
no dependency on a maker loop running.

### Why the chain can answer this at all

Every swap's ETH leg goes through one contract — the taker **locks ETH
first**, always — and `contracts/src/EthHTLC.sol` emits exactly the three
events a count needs:

```solidity
event Locked(bytes32 indexed swapId, address indexed sender, address indexed recipient,
             uint256 amount, bytes32 hashlock, uint256 timelock, bytes32 takerLezAccount);
event Claimed(bytes32 indexed swapId, bytes32 preimage);
event Refunded(bytes32 indexed swapId);
```

From those alone: swaps **attempted** (a `Locked`), **completed** (a
`Claimed`), **refunded** (a `Refunded`), **still open** (a `Locked` with
neither), **distinct taker wallets** (distinct `Locked.sender`), and the
**per-maker split** (`Locked.recipient`). Globally, for every maker at once,
back to deployment.

### Only the current venue

The report targets the pinned Sepolia deployment
`0x351B0EA07739FA9F6769213927D7836a790A5FAF` (`INTERFACE_VERSION=2`, deployed
in block `11417462`) — the address `deploy/maker.env.example` and
[`docs/testnet.md`](testnet.md) both pin. That address and the measured block
range are printed on every run, in both output forms.

`docs/testnet.md` also records a superseded v1 deployment
(`0x8636Fe66DFee166589a913140f14d5F57394834A`, block 11316985). Its swaps are
a **different era** and are never mixed in — adding `takerLezAccount` to
`lock()` changed `Locked`'s topic0, so the two cannot even be queried
together. Two guards make an era mix-up impossible rather than merely
unlikely:

- the same `INTERFACE_VERSION` handshake every client runs at startup
  (`eth::client::verify_interface_version`) runs here too, *before* any log
  query. The v1 contract has no such getter, so pointing the report at it
  fails loudly instead of quietly reading zero — the "goes deaf, doesn't
  error" failure mode described in `src/eth/client.rs`;
- `--eth-htlc-address` may override the venue, but a non-pinned address may
  **not** inherit the pinned venue's deployment block: the run is refused
  until an explicit `--from-block`/`--since` says where that deployment's own
  history starts.

### Operator runbook

```
# The whole trial, from the venue's deployment block to the chain head:
swap-cli chain-report --eth-rpc-url "$ETH_RPC_URL"

# Machine-readable:
swap-cli --json chain-report --eth-rpc-url "$ETH_RPC_URL"

# A trial period, measured after the fact — no baseline snapshot needed:
swap-cli chain-report --eth-rpc-url "$ETH_RPC_URL" --since 2026-08-21 --until 2026-09-04

# Or by block, when you want an exactly reproducible window:
swap-cli chain-report --eth-rpc-url "$ETH_RPC_URL" --from-block 11417462 --to-block 11535191
```

`--eth-rpc-url` reads `ETH_RPC_URL`, the same variable the maker and taker
use, so with the deployment's env loaded the command is just
`swap-cli chain-report`. `--since`/`--until` take a Unix timestamp,
`YYYY-MM-DD`, or `YYYY-MM-DDTHH:MM:SS` (UTC) and are resolved to block
numbers, which the report prints.

Unlike `maker --ops-report`, this command needs **no keys and no container
access** — not an `ETH_PRIVATE_KEY`, not a LEZ account, nothing. Anyone can
run it against the public endpoint and get the same numbers.

### Verified output

Run against live Sepolia on 2026-08-21 (the real `target/debug/swap-cli`, not
a mock):

```console
$ swap-cli chain-report --eth-rpc-url wss://ethereum-sepolia-rpc.publicnode.com
On-chain swap report — EthHTLC 0x351B0EA07739FA9F6769213927D7836a790A5FAF (pinned public-trial venue)
  chain id 11155111, INTERFACE_VERSION 2
  locks counted:    blocks 11417462–11535191 (inclusive)
  outcomes through: block 11535191

  attempted (ETH locked):  11
  completed (claimed):     6
  refunded:                2
  still open:              3
  distinct taker wallets:  2

  by maker (Locked.recipient):
    0x5000b8b08D987548b1fDbD594EDE7DCD809053d6  attempted 1  completed 0  refunded 0  open 1
    0x65dfE0D888e174695FcD2BAbf15c0FbAC69dc3Ab  attempted 7  completed 4  refunded 2  open 1
    0xB6ed858ab73bdb95Ad30eD8d19039E9d6A4ffeEF  attempted 1  completed 1  refunded 0  open 0
    0xbB6bb13e666b59985f69EA2959267F7B435fc2Cd  attempted 1  completed 0  refunded 0  open 1
    0xC3dbBE02A44241F027C490051FdFFbf54B06Dc62  attempted 1  completed 1  refunded 0  open 0
```

Cross-checked independently against the same endpoint with a plain
`eth_getLogs` sweep (union over repeated passes, to defeat the endpoint
flakiness described below): 11 `Locked`, 6 `Claimed`, 2 `Refunded`, 2 distinct
`Locked.sender` — identical.

The JSON form carries the same provenance:

```json
{"contract_address":"0x351B0EA07739FA9F6769213927D7836a790A5FAF","is_pinned_venue":true,
 "chain_id":11155111,"interface_version":2,"from_block":11417462,"to_block":11535191,
 "outcomes_to_block":11535193,"corroborated":true,"attempted":11,"completed":6,
 "refunded":2,"still_open":3,"distinct_taker_wallets":2,
 "by_maker":[{"recipient":"0x5000b8b08D987548b1fDbD594EDE7DCD809053d6","attempted":1,
              "completed":0,"refunded":0,"still_open":1}, …]}
```

### Reading it

- **How many tried?** → `attempted`. Every taker who locked ETH at the venue
  in the window, whether or not any maker answered. This is the denominator
  the ops ledger cannot produce.
- **How many completed?** → `completed`. The taker revealed the preimage and
  the ETH was claimed, i.e. both legs settled.
- **How many got their money back?** → `refunded`. Not a synonym for
  "failure": it is also the honest outcome of a taker who locked, found no
  counterparty, and waited out the timelock. For *why*, the ops ledger's
  `failed_by_code` is the only source ([§2](#2-the-ops-ledger--why-a-swap-failed)).
- **How many are stuck right now?** → `still open`. Derived, never observed:
  `attempted − completed − refunded`, i.e. ETH sitting in escrow as of
  `outcomes_to_block`. A swap mid-flight looks the same as one waiting out a
  timelock; re-run the report later to tell them apart.
- **Which maker served what?** → the per-maker rows, keyed on
  `Locked.recipient`. Each maker deployment has a distinct
  `ETH_RECIPIENT_ADDRESS` (a hard rule in `deploy/maker.env.example`), so the
  rows are per-maker, not per-operator.

`outcomes_to_block` is normally the chain head even when `--to-block`/
`--until` bound the attempt window. That is deliberate: a swap locked in the
last block of a trial window and claimed the next day is **completed**, not
"still open" — cutting the outcome scan at the window boundary would
systematically overstate `still open` at the end of every window.

### What this number does NOT mean

- **`distinct taker wallets` is a WALLET count, not a person count.** One
  person with two funded Sepolia keys is two; one shared key is one; a person
  who reinstalls and generates a new key is two. It is deliberately not
  refined further — see [Privacy boundary](#privacy-boundary).
- **Per-maker rows do not dedupe takers.** Their `attempted` values sum to the
  global `attempted`, but a taker who tried two makers appears under both, so
  the per-maker rows carry **no** taker count at all — summing one would
  double-count that person's wallet. The distinct-wallet figure exists only at
  the top level, and only there is it meaningful.
- **It counts swaps, not sessions.** A taker who locks, refunds, and retries
  is two attempts, by design: each attempt is a fresh hashlock (the LEZ escrow
  is one-per-hashlock and a burned secret cannot be reused — see
  `classify_candidate` in `src/swap/maker.rs`).
- **It cannot see the earlier funnel.** Someone who installs the module,
  requests a quote, and closes the app before locking anything leaves nothing
  on chain. Release-asset downloads
  ([§4](#4-release-asset-downloads-a-ceiling-not-a-count)) are the only (very
  rough) signal for that stage.
- **It counts the ETH leg.** A swap is only atomic across both chains, but the
  ETH lock is what every swap starts with, so the ETH leg is the complete
  attempt set. A `Claimed` means the ETH moved and the preimage is public —
  which is what makes the LEZ side claimable — so `completed` is a sound
  proxy for a settled swap, not a guess.

### A note on RPC endpoints (why the report may refuse to answer)

The pinned `ETH_RPC_URL` (`wss://ethereum-sepolia-rpc.publicnode.com`) is a
load-balanced pool that mixes a full-history Geth with a reth whose receipts
are pruned. For an identical historical `eth_getLogs`, the pruned node returns
`[]` — **no error, no warning**, an empty array. Repeating the same query
against it returns a different count roughly half the time.

For a number that is supposed to be the trial's headline, reading low
silently is the worst possible failure: it is indistinguishable from a quiet
trial. So the report proves its answers instead of trusting them. Each query
is batched with two tiny **canary** queries — one log at the oldest block of
the scan, one at the newest. A JSON-RPC batch is answered by a single backend,
and log retention is a contiguous block interval, so a backend that serves
both anchors provably covers everything between *them*. Note the exact claim:
the proven interval is `[oldest anchor, newest anchor]`, and each anchor is the
first block *with logs* found walking inward from its end — up to
`CANARY_PROBE_BLOCKS - 1` (15) blocks inside the scanned range. A pruning
boundary landing in that 16-block margin at either end is the one gap this
does not close. (This is also why the
report talks HTTP rather than `wss://`: alloy's WebSocket transport splits a
batch into separate sends, which would let the canary and the query it vouches
for be answered by different nodes. Same host, same configured endpoint.)

If the endpoint cannot corroborate a range — after retrying, reconnecting, and
drawing a fresh backend several times — the command **fails with an
explanation instead of printing a count**.

The same applies when the anchors cannot be found in the first place. A probe
that comes back **empty proves nothing**: it is what a chain with no logs looks
like *and* what a backend that has pruned this era's log index looks like, and
the response does not say which. Only a successful, non-empty probe counts as
evidence that the endpoint holds these blocks at all. Both ends are always
probed, and the two ways that can fail are **not** the same thing:

- **One end anchored, the other did not.** This is not ambiguous — the backend
  demonstrably holds logs for one end of the range and demonstrably would not
  produce any for the other, which is what a partial log index looks like. The
  run is **always refused**, and `--allow-uncorroborated` does not apply to it:
  the chain plainly has logs, so there is nothing for the operator to assert.
  The refusal names which end failed.
- **Neither end anchored.** Nothing was learned either way. The run is refused
  by default for the same reason as above — its likeliest wrong answer is a
  zero, and a zero from a pruned backend reads exactly like a quiet trial — but
  this is the one case a localnet genuinely produces, so it is yours to assert
  with `--allow-uncorroborated`, which downgrades the refusal to a printed
  `NOTE` and `"corroborated": false` in the JSON.

Either way, point `--eth-rpc-url` at a full-history endpoint, or narrow the
window to blocks it still retains. `"corroborated"` is never `false` unless
someone passed that flag; without it, a published count is always a proven one.
(A window that selects no blocks — `--since` in the future — queried nothing,
so nothing went unproven and the field stays `true` alongside its zeroes.)

---

## 2. The ops ledger — *why* a swap failed

The ops ledger is **not** the swap count any more; [§1](#1-the-on-chain-count--every-attempt-from-the-chain-itself)
is. What it still does, and nothing else can, is explain a failure: it records
a bounded [`FailureCode`](#what-gets-recorded-and-what-never-can) for every
swap that ended badly, which the chain has no way to represent — on chain a
stuck swap is just a `Refunded` or a `Locked` with no terminal event, whatever
the reason. Read it *after* §1 tells you something went wrong, to find out
what.

Its `accepted_total` remains meaningful, with a narrower meaning than it used
to be given: **swaps this maker accepted**, which is the subtrahend in the
[attempted-vs-accepted gap](#the-attempted-vs-accepted-gap) — not the number
of people who tried.

### Why this mechanism exists at all

Issue #98 asks specifically for something the maker **operates**: "append one
durable, versioned operation record when a signed acceptance reserves a maker
offer," "expose an operator command... unique counts grouped by `accepted`,
`completed`, `refunded`, and `failed`," "recover the ledger across
container/process restarts." That is not a GitHub-issue-count question — it
is an operations question about the standing liquidity bot
(`swap-cli maker --loop`), which is the process that actually serves every
public-trial swap on the offer board. So this is implemented as a new durable
**operations ledger** (`src/ops.rs`), not as another GitHub query:

- **A labelled GitHub issue/form count** (already live via #94's
  `trial-feedback` label — see [§3](#3-github-feedback-trial-feedback-label))
  is real, exact, and chain-checkable, but it only counts people who filed an
  issue. It cannot answer "how many swaps were accepted" — issue #98
  explicitly separates the two: *"GitHub feedback submissions remain
  qualitative and are not presented as swap-attempt telemetry."*
- **Release-asset download counts** ([§4](#4-release-asset-downloads-a-ceiling-not-a-count))
  are a real, already-live number (some `.lgx` assets show 70+ downloads),
  but they count installs/fetches, not swaps, and include bot/CDN traffic
  and re-downloads of the same asset by the same person. They are kept as a
  documented *ceiling*, never presented as a swap count.
- **The ops ledger** separates `accepted` (a taker's ETH lock was matched and
  the maker durably reserved its offer for that swap) from the **terminal**
  outcome (`completed` / `refunded` / `failed`), and attaches a bounded reason
  code to the last of those. The reason code is the part that is still
  irreplaceable.

What was *not* obvious when this was written, and is the reason §1 now leads:
the pinned EthHTLC contract answers "how many tried" better than any ledger
can, because the taker's ETH lock is on a public chain before any maker is
involved. The ledger was the best available source only for as long as nobody
had queried the venue's logs.

### Why hashlock + swap id, not `acceptance_id`

Issue #98's language ("the protocol's opaque `acceptance_id`") anticipates a
**signed** offer/acceptance handshake. That protocol does not exist yet in
this repo — `grep -r acceptance_id src` finds nothing; v1 offers are
unsigned, exactly as the issue's own "Release relationship" section
anticipates ("unsigned v1 offers do not provide the stable acceptance
identity needed for trustworthy deduplication"). Until that lands, the
ledger uses the identifiers that already are stable and protocol-native
today:

- the **hashlock** — the LEZ HTLC program derives exactly one escrow PDA per
  hashlock and refuses a second lock against an existing one, so it is
  already the de facto one-swap-per-hashlock identity (see
  `classify_candidate`'s doc in `src/swap/maker.rs`), and
- the **EthHTLC swap id**.

When a signed-acceptance protocol lands, only the dedupe key needs to change
(to `acceptance_id`); the record shape, the CLI surface, and the runbook
below do not.

### What gets recorded, and what never can

Implementation: `src/ops.rs` (`OpsLedger`, `OpsRecord`, `FailureCode`). One
append-only JSONL file (default `maker-ops.jsonl`, sibling of
`--state-file`), replayed into an in-memory map on load.

An `OpsRecord` has exactly these fields — nothing else:

```rust
pub struct OpsRecord {
    pub version: String,       // "trial-ops/1"
    pub hashlock: String,      // 64-hex, on-chain-public
    pub swap_id: String,       // 0x + 64-hex, on-chain-public
    pub release: String,       // this maker binary's CARGO_PKG_VERSION
    pub accepted_at: u64,
    pub final_at: Option<u64>,
    pub state: RecordState,    // Accepted | Completed | Refunded | Failed{code}
}
```

`FailureCode` is a **bounded enum** (`EthClaimUnresolved`,
`LezRefundUnresolved`, `PartialLockWedge`, `PreLockAbandoned`,
`CorruptEntry`, `Other`) — never a free-text error field. There is
structurally no field for a wallet/counterparty address, a peer id, an IP, a
messaging topic, a preimage, a private key, a mnemonic, an RPC URL, or
unbounded/raw error text. `src/ops.rs`'s test suite includes a fixture that
feeds an ETH address, a raw error string, and a credentialed URL through the
recorder's `hashlock`/`swap_id` parameters and asserts none of it is ever
persisted (`non_hex_shaped_input_is_rejected_not_persisted`) — the recorder
validates hex shape/length before writing anything.

### How this differs from the crash-recovery journal

`cli::bot::StateStore` (the existing `--state-file` journal) exists purely
for **fund safety**: it tracks only *currently in-flight* swaps and forgets
an entry the instant it reaches a confirmed terminal state, so a restart
never re-locks a resolved escrow. That means it is structurally unable to
answer "how many swaps have we ever served" — completed/refunded history
simply disappears from it. The ops ledger is the durable, append-only
complement: it never deletes anything, and it durably records `accepted`
strictly **after** the fund-safety journal write succeeds, so an ops-ledger
I/O failure can never gate or corrupt a swap in progress — only under-count
it, and even that self-heals the next time the same hashlock resolves (the
terminal write is idempotent and retried by whichever process — the
original run or a restart's `reconcile()` — next observes it).

### Idempotency and restart survival, proven

- A **replayed acceptance** or **duplicate terminal notification** never
  changes the count for that hashlock — the ledger's fold is
  insert-if-absent for `Accepted` and only-if-currently-`Accepted` for
  `Terminal`.
- **Accepted-then-refunded** and **accepted-then-failed** are counted in
  distinct buckets.
- A **restart between acceptance and terminal state** preserves exactly one
  in-flight (accepted, not yet terminal) trial; the restarted process's
  `reconcile()` pass — the same one that resolves the fund-safety journal —
  records the eventual terminal outcome once, whichever way it resolves.
- A **forged/out-of-order terminal event with no prior acceptance** (a
  malformed or untrusted message) is dropped, never fabricating an
  `accepted` count.
- A **corrupt/truncated JSONL line** (e.g. a crash mid-write) is skipped on
  load with a warning; it never poisons the rest of the ledger or crashes
  the maker.

All of the above are unit tests in `src/ops.rs` (`cargo test --lib ops::`)
and were also verified against the actual built `swap-cli` binary — see
[Verified output](#verified-output) below.

### Operator runbook

```
# How many distinct trial swaps were accepted, and how did each end?
swap-cli maker --ops-report --ops-file .maker-ops.jsonl

# Same, machine-readable:
swap-cli maker --ops-report --json --ops-file .maker-ops.jsonl
```

`--ops-file` defaults to `maker-ops.jsonl` next to `--state-file` (override
via `MAKER_OPS_FILE`), so on the deployed liquidity bot the real invocation
is just:

```
swap-cli maker --ops-report
```

(`--ops-report` is read-only — no network clients are constructed, and it
does not require the loop to be running.)

The report:

```json
{
  "accepted_total": 5,
  "completed": 2,
  "refunded": 1,
  "failed": 1,
  "failed_by_code": [["eth_claim_unresolved", 1]],
  "in_flight": 1
}
```

Reading it:

- **How many did THIS maker accept?** → `accepted_total`: every distinct
  hashlock this maker ever reserved a swap for, whether or not it finished.
  It is *not* the answer to "how many tried" — a taker no maker answered never
  appears here at all. For attempts, use
  [§1](#1-the-on-chain-count--every-attempt-from-the-chain-itself); the
  difference between the two is the
  [attempted-vs-accepted gap](#the-attempted-vs-accepted-gap).
- **How many completed?** → `completed`. Both sides of the swap settled: the
  taker claimed LEZ (revealing the preimage) and the maker claimed ETH.
- **How many hit problems, and why?** → `refunded + failed`, split further by
  `failed_by_code` (`eth_claim_unresolved`, `partial_lock_wedge`, etc. — see
  `FailureCode` above). **This is the reason to read this report at all**: the
  chain can tell you a swap was refunded, never why. `refunded` is not necessarily a failure
  in the pejorative sense — it is also the outcome of an honest taker who
  simply never showed up to claim before the LEZ timelock expired.
- **How many are still running?** → `in_flight` (`accepted_total -
  completed - refunded - failed`). A non-zero value after the loop has been
  up for a while, with no restart in progress, is worth investigating with
  `swap-cli maker --status`.

The standing loop also prints the durable, all-restarts ops-ledger totals
next to its own in-memory (this-process-uptime-only) `completed`/`failed`
counters when it stops, precisely so the two are never confused:

```
Maker loop stopped: 2 completed, 1 failed (this process's uptime)
Durable ops ledger (all-time, /path/to/maker-ops.jsonl): accepted=5 completed=2 refunded=1 failed=1 in_flight=1
```

### Verified output

Ran against a synthetic ledger (2 completed, 1 refunded, 1 failed, 1
in-flight — the built `target/debug/swap-cli`, not a mock):

```
$ swap-cli --env-file .env maker --ops-report --ops-file maker-ops.jsonl
Ops ledger: /tmp/.../maker-ops.jsonl
  accepted (distinct swaps ever reserved): 5
  completed:                               2
  refunded:                                 1
  failed:                                   1
    - EthClaimUnresolved: 1
  in-flight (accepted, no terminal state yet): 1

$ swap-cli --json --env-file .env maker --ops-report --ops-file maker-ops.jsonl
{"accepted_total":5,"completed":2,"refunded":1,"failed":1,"failed_by_code":[["eth_claim_unresolved",1]],"in_flight":1}
```

Then, to prove idempotency/forgery-resistance live (not just in unit
tests): appended (a) a **replayed** `accepted` line for hashlock `1111...`
(already terminal-`completed`) and (b) a **forged terminal** line for a
hashlock (`6666...`) that was **never** accepted, and re-ran the same
command — the totals were byte-for-byte identical (`accepted_total` stayed
`5`, not `6`; `completed` stayed `2`, not flipped or double-counted).

### What this number does NOT mean

- It counts **swaps**, not **people** — a person can accept, abandon, and
  retry, which increments `accepted_total` more than once (a fresh hashlock
  per attempt is required by design: the LEZ escrow is one-per-hashlock, and
  a rejected/expired attempt burns the secret — see `classify_candidate`'s
  doc). This is deliberate: issue #98 explicitly rules out fingerprinting a
  wallet/peer/IP to approximate a person-count.
- It only counts swaps against **this maker's own** liquidity bot instance.
  If more than one maker is standing on the board — the fleet runs five —
  each has its own `maker-ops.jsonl` and the totals need to be summed by hand
  across operators/hosts. §1 needs no such summation: one venue, one query,
  every maker at once.
- The GUI (Basecamp `swap_ui`) maker-role path
  (`swap_ffi_run_maker_loop`) writes to its own local
  `maker-ops.jsonl` next to its own `--state-file`/wallet home — it is not
  centrally aggregated with the deployed liquidity bot's ledger. Pull both
  files (`scp`/copy) if a tester ran maker-role locally and you want their
  numbers folded in.
- `accepted_total` only counts a swap once the maker's ETH-lock watcher
  *matched and reserved* it. Two whole populations are therefore missing:
  a taker who opened the app, requested a quote and closed it before locking
  anything (nothing on chain, nothing anywhere — release-asset downloads,
  [§4](#4-release-asset-downloads-a-ceiling-not-a-count), are the only rough
  signal), **and** a taker who did lock ETH but whom no maker ever answered.
  The second group is invisible here and fully visible in
  [§1](#1-the-on-chain-count--every-attempt-from-the-chain-itself) — which is
  precisely why §1 leads.

---

## 3. GitHub feedback (`trial-feedback` label)

PR #94's issue template (`.github/ISSUE_TEMPLATE/trial-feedback.yml`) applies
the `trial-feedback` label automatically. Count and read submissions with:

```
# How many people filed trial feedback?
gh issue list --repo logos-co/eth-lez-atomic-swaps --label trial-feedback --state all

# Read one:
gh issue view <number> --repo logos-co/eth-lez-atomic-swaps
```

**Verified live** (2026-08-05):

```
$ gh issue list --repo logos-co/eth-lez-atomic-swaps --label trial-feedback --state all
(no output — zero issues currently carry this label)
```

Zero so far — the label and template exist and are wired correctly, but no
public tester has filed one yet. This count will only ever go up when
someone chooses to file; it is a real, exact, chain-checkable-if-they-
include-evidence number, but it is a **self-selected sample**, never a
denominator. Use it for qualitative color ("what confused people," "did
anyone report a bug") next to the ops ledger's quantitative attempt/success
counts.

---

## 4. Release asset downloads (a ceiling, not a count)

```
# Per-release asset download counts:
gh api repos/logos-co/eth-lez-atomic-swaps/releases -q \
  '.[] | .tag_name as $t | .assets[] | [$t, .name, .download_count] | @tsv'
```

**Verified live** (2026-08-05):

```
swap_ui-v0.3.3  sidecar.json           0
swap_ui-v0.3.3  swap_ui-0.3.3.lgx      0
swap_ui-v0.3.2  sidecar.json           0
swap_ui-v0.3.2  swap_ui-0.3.2.lgx      13
swap-v0.3.3     sidecar.json           0
swap-v0.3.3     swap-0.3.3.lgx         5
swap-v0.3.2     sidecar.json           1
swap-v0.3.2     swap-0.3.2.lgx         13
swap_ui-v0.3.0  sidecar.json           3
swap_ui-v0.3.0  swap_ui-0.3.0.lgx      19
swap-v0.3.1     sidecar.json           5
swap-v0.3.1     swap-0.3.1.lgx         18
swap-v0.3.0     sidecar.json           2
swap-v0.3.0     swap-0.3.0.lgx         16
swap_ui-v0.2.0  sidecar.json           1
swap_ui-v0.2.0  swap_ui-0.2.0.lgx      70
swap-v0.2.0     sidecar.json           3
swap-v0.2.0     swap-0.2.0.lgx         73
```

Sum of the `.lgx` module-artifact downloads across every release so far:
`0+13+5+13+19+18+16+70+73 = 227` (swap + swap_ui combined).

This is a **ceiling on installs**, never a swap count and never a person
count:

- it includes automated/CDN/mirror fetches and GitHub's own asset-serving
  infrastructure, not just human testers;
- lgpm/scaffold re-resolving a pin re-fetches the same asset, so one person
  iterating on their own setup can inflate this a lot;
- a download is not an install, and an install is not a launch, let alone an
  attempted swap.

Use it only as a rough, already-live upper bound on "how many times has a
build of this module been fetched," never in the same sentence as "how many
people tried it" without that caveat attached.

---

## Privacy boundary

None of the four mechanisms above can ever contain: a mnemonic, private key,
password, hashlock **preimage**, raw configuration, a peer id, an IP address,
a messaging topic, or unbounded/raw error text. Nothing anywhere here makes an
outbound call the app would not otherwise make, and nothing is written
anywhere by the reporting paths.

- The on-chain report reads data that is already public — anyone can query
  the venue's logs — but it still **aggregates rather than lists**. Taker
  addresses (`Locked.sender`) are collapsed to a distinct-wallet *count* and
  never appear in any output, default or `--json`; `src/eth/report.rs` has a
  test (`serialized_counts_carry_no_taker_address`) asserting exactly that on
  a serialized report. The only addresses printed are the `Locked.recipient`
  maker/venue addresses the per-maker split is about, which are operator-owned
  and already published in `deploy/`. No chain-derived address is ever written
  into the ops ledger's record shape — the report writes no files at all.
- Adding the chain source deliberately did **not** loosen the ops ledger's
  boundary: §1 aggregates, §2 still cannot represent an address, and the two
  are not joined on anything but counts.
- The ops ledger enforces this structurally (§2: `OpsRecord`'s field set,
  `FailureCode`'s bounded enum, and the hex-shape validation on every write)
  and is exercised by `src/ops.rs`'s test suite.
- The GitHub feedback flow enforces its own, separate `trial-evidence/1`
  public-evidence-key allowlist in CI (PR #94) — see
  `tests/check-feedback-evidence.mjs`. The two allowlists are independent by
  design (`SCHEMA_VERSION`/`trial-ops/1` vs `trial-evidence/1`) so neither
  has to change when the other does.

Transaction evidence (tx hash, explorer link) stays in the user-owned
receipt flow and the public feedback form's `trial-evidence/1` object; it is
deliberately **not** duplicated into the durable ops ledger, which only ever
stores the hashlock, swap id, release, timestamps, and bounded outcome/code.
The on-chain report does not emit tx hashes either — it reports counts, not
receipts.
