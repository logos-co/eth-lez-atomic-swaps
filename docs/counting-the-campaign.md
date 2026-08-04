# Counting the public trial (issue #98)

The owner's ask (issue #62 / #98): *"a CTA at the end — something that helps
me track how many ppl actually tried, and if they have feedback."*

PR #94 shipped the **feedback** half: a Basecamp-compatible public-trial form
(`.github/ISSUE_TEMPLATE/trial-feedback.yml`) with "Copy safe evidence" /
"Copy feedback link" end-state actions. This document covers the **counting**
half — the durable, queryable answer to "how many people actually tried
this, and how did it go?" — plus the runbook for both.

There is **no telemetry** anywhere in this project and this feature adds
none: every number below comes from either a file the maker operator already
owns, or a `gh` query against data the operator (or a tester) put on GitHub
voluntarily. Nothing here makes an outbound call the app wouldn't otherwise
make.

## Three numbers, three very different meanings

| # | Mechanism | What it counts | Kind of number |
|---|-----------|-----------------|-----------------|
| 1 | Ops ledger (`swap-cli maker --ops-report`) | Distinct swaps the maker's standing liquidity bot actually **accepted and served** | Exact, durable, chain-checkable |
| 2 | `trial-feedback` GitHub issues | People who **chose to tell us** something | Exact count, but a self-selected sample |
| 3 | Release asset downloads | Times a `.lgx` module artifact was **fetched** | A ceiling only — includes bots/CDN/re-downloads |

None of these three is "number of distinct people who tried Atomic Swaps."
(1) counts swaps, not people, and deliberately does not attempt to
fingerprint a wallet/peer/IP to get closer to a person-count — see
[Privacy boundary](#privacy-boundary). Use (1) as the primary, trustworthy
signal; (2) as qualitative color; (3) only as an upper bound on installs.

---

## 1. The ops ledger — attempts vs completions

### Why this mechanism, and why not the alternatives

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
  `trial-feedback` label — see [§2](#2-github-feedback-trial-feedback-label))
  is real, exact, and chain-checkable, but it only counts people who filed an
  issue. It cannot answer "how many swaps were accepted" — issue #98
  explicitly separates the two: *"GitHub feedback submissions remain
  qualitative and are not presented as swap-attempt telemetry."*
- **Release-asset download counts** ([§3](#3-release-asset-downloads-a-ceiling-not-a-count))
  are a real, already-live number (some `.lgx` assets show 70+ downloads),
  but they count installs/fetches, not swaps, and include bot/CDN traffic
  and re-downloads of the same asset by the same person. They are kept as a
  documented *ceiling*, never presented as a swap count.
- **The ops ledger** is the only mechanism that can distinguish "40 people
  installed, 3 completed a swap" from "3 installed, 3 completed" — which is
  exactly the distinction the task calls out. It does this by recording
  `accepted` (a taker's ETH lock was matched and the maker durably reserved
  its offer for that swap) separately from the **terminal** outcome
  (`completed` / `refunded` / `failed`).

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

- **How many tried?** → `accepted_total`. This is the attempt count — every
  distinct hashlock the maker ever reserved a swap for, whether or not it
  finished. This is the number that tells "40 installed, 3 completed" apart
  from "3 installed, 3 completed": both scenarios show `completed: 3`, but
  only the first shows `accepted_total: 40` (or however many actually got as
  far as locking ETH — see the caveat below).
- **How many completed?** → `completed`. Both sides of the swap settled: the
  taker claimed LEZ (revealing the preimage) and the maker claimed ETH.
- **How many hit problems?** → `refunded + failed`, split further by
  `failed_by_code` for *why* (`eth_claim_unresolved`, `partial_lock_wedge`,
  etc. — see `FailureCode` above). `refunded` is not necessarily a failure
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
  If more than one maker is standing on the board, each has its own
  `maker-ops.jsonl` and the totals need to be summed by hand across
  operators/hosts.
- The GUI (Basecamp `swap_ui`) maker-role path
  (`swap_ffi_run_maker_loop`) writes to its own local
  `maker-ops.jsonl` next to its own `--state-file`/wallet home — it is not
  centrally aggregated with the deployed liquidity bot's ledger. Pull both
  files (`scp`/copy) if a tester ran maker-role locally and you want their
  numbers folded in.
- `accepted_total` only counts a swap once the maker's ETH-lock watcher
  *matched and reserved* it — a taker who opens the app, requests a
  quote, and closes it before locking anything on Ethereum never appears
  here (there is nothing on-chain to key a durable, verifiable record on).
  Release-asset downloads ([§3](#3-release-asset-downloads-a-ceiling-not-a-count))
  are the only (rough) signal for that earlier funnel stage.

---

## 2. GitHub feedback (`trial-feedback` label)

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

## 3. Release asset downloads (a ceiling, not a count)

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

None of the three mechanisms above can ever contain: a mnemonic, private
key, password, hashlock **preimage**, raw configuration, a wallet or
counterparty address, a peer id, an IP address, a messaging topic, a
transaction hash, a full receipt, or unbounded/raw error text.

- The ops ledger enforces this structurally (§1: `OpsRecord`'s field set,
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
