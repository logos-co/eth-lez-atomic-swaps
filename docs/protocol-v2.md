# Atomic Swaps signed protocol v2

- Status: corrected freeze candidate; independent re-review required
- Applies to: public Sepolia ↔ LEZ testnet mode only
- Repository baseline: `80f36e9a67530ad20995084f2f6ca04a9be350b4`
- Date: 2026-08-04

## Decision

**Specification correction result: READY FOR INDEPENDENT RE-REVIEW.** The exact type strings, corrected deterministic EIP-712 vectors, signatures, recovered addresses, interface-v2 deployment pin, and six-field swap-ID derivation are executable through the fixture gate. A reviewer who did not implement this correction MUST independently reproduce them before this candidate can be frozen. This document retains the normative, code-facing rules that address the previously identified Critical and High protocol-design findings.

**Public-mode implementation result: NO-GO.** Public-mode implementation and enablement remain blocked until the release-specific values and external capabilities listed under [Remaining blockers](#remaining-blockers) are approved. Code may not substitute a URL, display name, zero, current config, or a self-signed value for a missing release pin. No v1 or on-chain-only fallback is permitted.

The words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.

## Security invariant

The only operation allowed to authorize maker LEZ inventory is:

```text
strict packet admission
-> trusted signed-offer verification
-> signed-accept verification and exact offer linkage
-> canonical Ethereum receipt/log/state verification at 3 confirmations
-> exact policy, inventory, and LEZ absence checks
-> exclusive durable reservation of offer ID, ETH swap ID, and hashlock
-> immediate canonical Ethereum recheck
-> exact LEZ lock/fund from the immutable journal snapshot
```

Rust owns that complete transition. Delivery/Waku, the Node child, C++, Qt, QML, JSON, event watchers, and RPC notifications are untrusted inputs. An event may wake a verifier; it may never authorize funding.

For every rejection, error, timeout, expiry, cache eviction, process failure, RPC ambiguity, or missing acceptance, the observable count of LEZ lock/fund calls MUST remain zero unless a durable `VerifiedReserved` record already exists for that exact immutable snapshot.

## Fixed public-testnet policy

The release policy object is immutable for a running process and MUST be loaded before Delivery starts:

| Field | Required value/policy |
|---|---|
| Protocol | `2` only |
| Ethereum network | Sepolia, chain ID `11155111` |
| Ethereum confirmations | `3`, including the transaction's block |
| Reorg replay window | `256` Ethereum blocks |
| ETH HTLC | `0x351b0ea07739fa9f6769213927d7836a790a5faf` |
| ETH HTLC interface | `INTERFACE_VERSION = 2` |
| ETH HTLC runtime code hash | `0xbad9367560aa868d44420e15b958ad1c5644cdd20ef4bed85af4d1c33d3fa1a2` |
| LEZ HTLC program ID | `0x9eb88f51aae87a58fb74b8d2dc7327b39333585e63280e3f9cf8d86dac0ed702` |
| LEZ chain ID | one nonzero authoritative 32-byte release pin; currently blocked |
| Maker offer signer | one pinned EOA; currently blocked |
| Maker ETH recipient | MUST equal the pinned offer signer for v2 trial |
| Maker LEZ account | one pinned 32-byte account; currently blocked |
| Offer amount | exactly `100000000000000` wei for exactly `150` LEZ |
| LEZ duration | exactly `1200` seconds |
| ETH duration | exactly `2400` seconds |
| Minimum margin | exactly `300` seconds |
| Minimum LEZ headroom at maker decision | `600` seconds |
| Offer TTL | exactly `120` seconds |
| Offer heartbeat | every `30` seconds while healthy |
| Accept TTL | exactly `180` seconds |
| Accept republish | every `5` seconds plus random `0..500` ms jitter |
| Maximum fills | exactly `1` |
| Allowed signer class | canonical low-`s` secp256k1 EOA only |

The Ethereum values above were freshly read on 2026-08-04 from two independent public providers, `https://ethereum-sepolia-rpc.publicnode.com` and `https://sepolia.drpc.org`: both returned chain ID `11155111`, `INTERFACE_VERSION = 2`, `minTimelockDelta = 300`, and the same runtime-code hash. Both providers returned 5,146 bytes of runtime code. The repository records deployment transaction `0x9ce42d59b141d8fd1759e2f288f11837dca335bb6cd4466e8fd9330c2b25e68f` at block `11417462` from source commit `d794f04`. Release acceptance still requires independent provenance review linking the reviewed build, deployment record, and runtime bytecode.

The LEZ program value is the repository's checked-in public-testnet ImageID and deploy target. It is a program identity, not a network identity.

### Release trust root and rotation

For v2 trial the trust root is an immutable `TrustedMakerV2` record compiled into the `swap` Rust core and repeated in the release manifest/catalogue metadata:

```text
TrustedMakerV2 {
  protocol_version: 2,
  release_id: nonzero bytes32,
  maker_offer_signer: address,
  maker_eth_address: address,
  maker_lez_account: bytes32,
  ethereum_chain_id: uint256,
  ethereum_htlc: address,
  ethereum_htlc_code_hash: bytes32,
  lez_chain_id: bytes32,
  lez_htlc_program_id: bytes32,
  valid_from: uint64,       // Unix seconds
  valid_until: uint64       // Unix seconds, at most 7 days after valid_from
}
```

The reviewed source, release approval, artifact checksum, and catalogue package checksum bind this record to the installed binary. If the release system later adds package signatures, they add supply-chain assurance but do not change the protocol check. Runtime configuration may narrow the compiled record but MUST NOT add or replace identities. Environment variables, offers, Delivery messages, DNS, and RPC responses cannot modify it.

For the one-maker trial `maker_offer_signer == maker_eth_address`. The recovered offer signer, offer field, on-chain ETH recipient, and pinned address MUST all equal.

Rotation or revocation requires a newly approved module/catalogue release. Rotation ships a new compiled record and `release_id`; revocation ships no active record. An old package fails closed after `valid_until`, limiting suppression of an update to seven days. It MUST display “maker trust record expired; update Atomic Swaps” and disable accepting offers. Release-distribution trust follows the catalogue's release-security process and is outside this wire protocol. A faster remote revocation mechanism is desirable, but no unsigned or TLS-only remote registry may override the compiled pin.

This is the selected trust model for v2. An open marketplace is a separate protocol mode and MUST NOT reuse the trusted-maker badge or one-click acceptance path.

## Chain and program identities

`ethereumChainId` is the numeric result of `eth_chainId` and is also the EIP-712 domain `chainId`.

`ethereumHtlcCodeHash` is `keccak256(runtime_bytecode)` from `eth_getCode(ethereumHtlc, block)` at the canonical verification block. Empty code is an error. Proxy resolution is not supported; the address itself MUST contain exactly the pinned runtime code.

`lezHtlcProgramId` is the 32-byte LEZ ImageID in the repository's established little-endian-per-`u32` wire order. JSON uses lowercase `0x` hex. LEZ transaction construction uses the same 32 bytes without reinterpretation.

`lezChainId` MUST be a stable 32-byte genesis/network commitment returned by an authoritative LEZ capability, independent of endpoint URL and operator. The capability contract is:

```text
get_network_identity() -> {
  chain_id: [u8; 32],
  genesis_commitment: [u8; 32]
}
```

For v2, `chain_id` MUST equal `genesis_commitment`; a later LEZ standard may define a distinct derivation only in a new protocol version. The value MUST be identical across two independently configured sequencer endpoints and equal the release pin before public mode starts. If this capability is absent, inconsistent, zero, malformed, or unavailable, public mode MUST stop. Hashing a URL, TLS certificate, display name, or program ID is forbidden.

## Topics and transport envelopes

```text
offers:  /atomic-swaps/2/offers/json
accepts: /atomic-swaps/2/accepts/json
```

The exact offer envelope keys are:

```json
{"type":"swap-offer/2","offer":{...},"signature":"0x..."}
```

The exact accept envelope keys are:

```json
{"type":"swap-accept/2","offer_envelope":{...},"accept":{...},"signature":"0x..."}
```

`offer_envelope` is the complete v2 offer object, including its maker signature. Verification always reconstructs typed values from it. Cached offer state is not authoritative.

### Strict JSON rules

- UTF-8 only, no BOM; root and nested values MUST be JSON objects.
- Unknown keys and duplicate keys at every level are errors. A parser that silently keeps the first or last duplicate is forbidden.
- Maximum decoded envelope size is 16,384 bytes, maximum Delivery/base64 encoded value is 24,576 bytes, maximum nesting depth is 4, and maximum JSONL line size is 32,768 bytes including newline.
- Security integers are quoted canonical base-10 strings: regex `0|[1-9][0-9]*`. Signs, decimals, exponents, leading zeroes, whitespace, and overflow are errors.
- `uint32`, `uint64`, `uint128`, and `uint256` are range-checked before construction. Arithmetic uses checked operations.
- Ethereum addresses are exactly 20 bytes rendered as 42 lowercase `0x` hex characters. All `bytes32` values are 66 lowercase `0x` hex characters and nonzero where stated.
- Signatures are 132 lowercase `0x` hex characters.
- LEZ account IDs are canonical base58 in JSON, with no leading/trailing whitespace, and MUST decode to exactly 32 bytes. Decode then re-encode; inequality is an error. The decoded bytes are signed as `bytes32`.
- QML/C++/Node MUST forward original bytes or base64 and MUST NOT deserialize/re-emit signed values. In particular no security integer may cross an IEEE-754 numeric representation.
- JSON formatting and key order are not signed. EIP-712 typed values are the only signed preimage.

### Exact offer object keys

```text
offer_id
ethereum_chain_id
ethereum_htlc
ethereum_htlc_code_hash
lez_chain_id
lez_htlc_program_id
maker_eth_address
maker_lez_account
eth_amount_wei
lez_amount
lez_timelock_duration_sec
eth_timelock_duration_sec
min_timelock_margin_sec
max_fills
issued_at
expires_at
```

### Exact accept object keys

```text
offer_id
offer_digest
ethereum_chain_id
ethereum_htlc
ethereum_htlc_code_hash
lez_chain_id
lez_htlc_program_id
maker_eth_address
maker_lez_account
eth_amount_wei
lez_amount
eth_swap_id
eth_lock_tx_hash
hashlock
taker_eth_address
taker_lez_account
eth_refund_after
lez_refund_after
issued_at
expires_at
```

`eth_refund_after` is Unix seconds. `lez_refund_after` is Unix milliseconds and is passed unchanged to the LEZ program.

## Exact EIP-712 contract

### Domain

The standard domain type string is exactly:

```text
EIP712Domain(string name,string version,uint256 chainId,address verifyingContract,bytes32 salt)
```

Values for both offer and accept are:

```text
name              = "Logos Atomic Swaps"
version           = "2"
chainId           = ethereumChainId
verifyingContract = ethereumHtlc
salt              = keccak256(abi.encode(
                      string("logos.atomic-swaps"),
                      uint256(2),
                      bytes32(lezChainId),
                      bytes32(lezHtlcProgramId)
                    ))
```

`abi.encode`, not packed encoding, is mandatory for the salt. The digest is:

```text
keccak256(0x1901 || domainSeparator || hashStruct(message))
```

### Offer primary type

The type string is exactly one line with no inserted whitespace:

```text
SwapOfferV2(bytes32 offerId,uint256 ethereumChainId,address ethereumHtlc,bytes32 ethereumHtlcCodeHash,bytes32 lezChainId,bytes32 lezHtlcProgramId,address makerEthAddress,bytes32 makerLezAccount,uint256 ethAmountWei,uint128 lezAmount,uint64 lezTimelockDurationSec,uint64 ethTimelockDurationSec,uint64 minTimelockMarginSec,uint32 maxFills,uint64 issuedAt,uint64 expiresAt)
```

Field order is the type-string order. `offerId` is a nonzero random 32-byte value created once per inventory slot and persisted before first publication. Heartbeats reuse the ID but update `issuedAt`, `expiresAt`, signature, and digest. Consuming any digest consumes the whole ID.

### Accept primary type

The exact type string is:

```text
SwapAcceptV2(bytes32 offerId,bytes32 offerDigest,uint256 ethereumChainId,address ethereumHtlc,bytes32 ethereumHtlcCodeHash,bytes32 lezChainId,bytes32 lezHtlcProgramId,address makerEthAddress,bytes32 makerLezAccount,uint256 ethAmountWei,uint128 lezAmount,bytes32 ethSwapId,bytes32 ethLockTxHash,bytes32 hashlock,address takerEthAddress,bytes32 takerLezAccount,uint64 ethRefundAfter,uint64 lezRefundAfter,uint64 issuedAt,uint64 expiresAt)
```

The duplicated offer fields MUST equal the verified nested offer's typed values. `offerDigest` MUST be recomputed from that nested offer. `ethSwapId`, `ethLockTxHash`, and `hashlock` are nonzero.

The current type authenticates the taker's Ethereum EOA but only carries a
self-declared `takerLezAccount`; it does not prove control of that LEZ account.
Public mode therefore remains blocked until a LEZ-native proof-of-possession
mechanism is specified, signed over this exact acceptance/offer binding, and
verified before reservation. Treating a syntactically valid account identifier
as possession is forbidden: it lets an attacker repeatedly strand the maker's
single inventory slot in an escrow nobody can claim.

### EOA signature rules

- Signature is exactly 65 bytes `r || s || v`.
- `v` MUST be 27 or 28; EIP-155 values and 0/1 are rejected.
- `r` and `s` MUST be nonzero and in curve range.
- `s` MUST be at most `secp256k1n / 2` (EIP-2 low-`s`).
- Recovered address MUST be nonzero and exactly match the declared/pinned role.
- EIP-2098, EIP-1271, personal-sign prefixes, and alternate encodings are unsupported in v2.

## Offer, accept, and timelock construction

### Maker offer

1. Complete journal reconciliation and acquire the exclusive maker-state lock.
2. Prove Delivery accept subscription readiness.
3. Read canonical Ethereum head timestamp `T` and verify the active trust record at `T`.
4. Create or load the durable live `offerId`.
5. Set `issuedAt = T`, `expiresAt = T + 120`, durations `1200/2400`, margin `300`, `maxFills = 1`, and exact release identities/amounts.
6. Sign in Rust with the pinned maker EOA. Send only the signed public envelope to the adapter/sidecar.
7. Repeat at 30-second intervals while the accept subscription, trust record, inventory, RPCs, and journal remain healthy.

If any liveness prerequisite fails, stop heartbeats. Existing journal recovery continues without Delivery.

### Taker lock and acceptance

1. Verify the offer signature, package trust pin, domains, policy, and freshness before enabling “Accept.”
2. Read a canonical Ethereum head with timestamp `T0`. Require `offer.issuedAt <= T0 <= offer.expiresAt`.
3. Persist the 32-byte preimage, SHA-256 hashlock, full signed offer, and `T0` before transaction preparation.
4. Set exact absolute times:

   ```text
   ethRefundAfter = T0 + offer.ethTimelockDurationSec
   lezRefundAfter = (T0 + offer.lezTimelockDurationSec) * 1000
   ```

   All operations are checked. These equations are later enforced by the maker.
5. Prepare and sign the Ethereum lock transaction for the exact sender, maker recipient, wei amount, hashlock, `ethRefundAfter`, and taker LEZ account. Compute its transaction hash and deterministic swap ID before broadcast; durably persist the raw signed transaction/hash/swap ID.
6. Broadcast, obtain a successful receipt containing exactly one matching `Locked` log, and persist receipt metadata.
7. Construct the accept with `issuedAt = T0` and `expiresAt = T0 + 180`. The accept signer MUST be the transaction sender. Persist the signed accept before reporting `EthLocked` or publishing.
8. Republish the identical signed accept every 5 seconds plus random `0..500` ms jitter. Do not change timestamps or signature. Stop at exact funded LEZ observation, `expiresAt`, canonical ETH refund, or fatal validation error.

The maker additionally requires:

- `accept.issuedAt == T0` and the two exact equations above.
- `offer.issuedAt <= accept.issuedAt <= offer.expiresAt`.
- `offer.issuedAt <= lockBlock.timestamp <= offer.expiresAt`.
- `accept.issuedAt <= lockBlock.timestamp <= accept.expiresAt`.
- At initial admission, canonical Ethereum head time is at most `accept.expiresAt`.
- `ethRefundAfter * 1000 >= lezRefundAfter + 300000`.
- Immediately before LEZ submission, at least 600,000 ms remain before `lezRefundAfter` using canonical Ethereum head time as the cross-chain conservative reference.

Once an accept is durably reserved while fresh, later accept/offer expiry does not discard recovery work. Chain headroom and exact on-chain state remain mandatory.

## Maker authorization API and exact order

Public mode MUST expose one Rust entry point with no alternate funding path:

```text
authorize_and_lock_v2(raw_accept_envelope) -> Result<FundedSnapshot, ProtocolError>
```

The current `EthHtlcEvent::Locked -> run_maker -> LezClient::lock` path MUST be unreachable in public mode. V1 is permitted only behind an explicit local-development mode that cannot use public release configuration.

The implementation order is fixed:

1. Apply byte, UTF-8, topic, depth, JSON-shape, duplicate-key, identifier, numeric, and cheap time checks. Invalid data is neither cached nor sent to RPC.
2. Strictly decode the nested offer. Compute its EIP-712 digest and enforce signature format. Require recovered signer = `makerEthAddress` = trusted maker signer = pinned maker ETH address.
3. Require every chain/contract/code/program/account/amount/duration/policy field to equal the active release record. Reject an inactive/expired trust record.
4. Strictly decode accept and compute its digest. Require recovered signer = `takerEthAddress`. Require exact nested digest, duplicate fields, time equations, TTL, and unused replay keys.
5. Only now admit the candidate into the bounded “signature valid, RPC pending” queue.
6. Query `eth_chainId` and exact runtime code at the candidate's address. Both MUST equal pins.
7. Fetch `ethLockTxHash` receipt. Require status 1, `to == ethereumHtlc`, canonical block number/hash, and exactly one decodable `Locked` log emitted by that contract. Missing or multiple matching logs are errors.
8. Require three confirmations, defined as `canonicalHead.number >= receipt.blockNumber + 2`. Re-fetch the receipt. Fetch its block by hash and by number; both lookups MUST return the same hash. Receipt block/hash/log index MUST be unchanged.
9. Recompute `ethSwapId = keccak256(abi.encodePacked(sender, recipient, amount, hashlock, timelock, takerLezAccount))`. Require exact log and accept equality: swap ID, sender/signer/taker, maker recipient, amount, hashlock, timelock, taker LEZ account, transaction hash, and contract.
10. Call `getHTLC(ethSwapId)`. Any RPC/decode error is fatal for this attempt. Require exact sender, recipient, amount, hashlock, timelock, taker LEZ account, and `OPEN` state.
11. Validate canonical chain time, exact duration equations, margin, 600-second LEZ headroom, maker inventory, and release/trust validity. Through the pinned LEZ network/program, derive the PDA and require authoritative absence of any escrow for this hashlock.
12. Enter the exclusive authorization section. A single process-wide mutex plus an exclusive OS file lock on the maker journal prevents same-host multi-process funding. Recheck in-memory and durable indexes for `offerId`, `ethSwapId`, `acceptDigest`, and hashlock. Exactly one candidate wins; deterministic tie-breaking is the first successful durable reservation.
13. Fsync a complete immutable `VerifiedReserved` snapshot and all four replay indexes using temp-file write, file fsync, atomic rename, and directory fsync. Any persistence error rolls back the in-memory reservation and produces zero LEZ calls.
14. Immediately repeat steps 6–10 and the headroom/trust checks. Receipt or block changes, RPC ambiguity, terminal state, or expiry of safety headroom moves the record to `RejectedReserved`/tombstone and produces zero LEZ calls.
15. Persist `LezCreateIntent`, then invoke the split/idempotent LEZ executor with the snapshot's exact maker, taker, amount, hashlock, program, and `lezRefundAfter`. After authoritative exact escrow observation persist `LezCreated`; before funding persist `LezFundIntent`; after exact balance and escrow verification persist `Funded`.
16. The post-fund verifier requires exact program owner, PDA, hashlock, maker, taker, amount, timelock, state `Locked`, preimage absent, and PDA balance exactly equal to the signed amount. `>=` is forbidden.
17. After a confirmed LEZ claim, verify the 32-byte preimage and hash, persist `LezClaimObserved`, and claim ETH with durable intent/retry. Only canonical Ethereum `CLAIMED` permits `EthClaimed` terminal state. If LEZ reaches canonical `Refunded`, persist `LezRefunded`. Ambiguous states remain active.

No caller may invoke the LEZ public-mode executor without a `VerifiedReserved` snapshot loaded from the durable store. The executor accepts typed snapshot IDs, not free-form config.

## Ethereum confirmation and reorg policy

- Three confirmations include the transaction block: inclusion is confirmation 1; two canonical descendants produce confirmation 3.
- Both pre-reservation and pre-LEZ checks require receipt status, contract code hash, exact log, block-by-hash, block-by-number, and `getHTLC == OPEN`.
- A receipt disappearing, changing block/log index, changing status, or pointing to a noncanonical block is `ETH_REORG` and produces no new LEZ action.
- Every RPC timeout, malformed result, disagreement, missing block, null receipt, or decode failure fails closed. An event watcher never substitutes for these reads.
- The watcher MUST replay at least 256 blocks and retain block/hash cursor history so it can report removals, but authorization remains receipt-driven.
- After LEZ is funded, a later Ethereum reorg is recovery work, not grounds to forget the swap. Retry canonical observation and escalate unhealthy; never create a second LEZ escrow.
- One configured RPC remains a residual infrastructure trust. Release acceptance MUST run the pre-funding reads against two independent Sepolia providers and require equality for chain ID, code hash, receipt block/hash, and HTLC state. Disagreement is fail-closed.

## Exact LEZ verification

Before the taker reveals the preimage, it MUST:

1. Verify the active release `lezChainId` through two endpoints and the exact `lezHtlcProgramId`.
2. Derive the PDA from the signed hashlock and program using the protocol's existing deterministic derivation.
3. Use a total, non-panicking decoder. Valid encoded lengths are exactly 125 bytes for `Locked`/`Refunded` with no preimage and 157 bytes for `Claimed` with a 32-byte preimage. State/preimage combinations outside those forms are invalid.
4. Require exact PDA ownership/program, hashlock, maker account, taker account, amount, `lezRefundAfter` milliseconds, state `Locked`, absent preimage, and balance equal to amount.

A `None`, short read, malformed state, unknown program owner, endpoint disagreement, or `>=`-only balance match produces no reveal and no claim.

## Bounded transport, cache, and sidecar

### Admission bounds

- Raw receive channel: 512 envelopes maximum; overflow drops the new envelope and sets unhealthy/backpressure.
- Signature-valid/RPC-pending accepts: 256 total, 4 per recovered taker, 1 per offer ID, 1 per ETH swap ID, and 1 per hashlock.
- Concurrent signature recoveries: 16.
- Token bucket: 16 signature checks/second, burst 32.
- Concurrent Ethereum verification tasks: 8.
- Token bucket: 4 new RPC validations/second, burst 8.
- Invalid envelopes never enter validated caches. Duplicate identical accepts refresh no timestamp and create no new work.
- Evict expired unreserved entries first, then oldest unreserved valid entry. Active, reserved, journaled, or consumed records are never evicted. If no safe victim exists, reject the new candidate with `CACHE_FULL` and set unhealthy.
- Delivery timestamps/message hashes are observability only.

### Sidecar machine protocol

Rust writes commands to child stdin; child stdout emits machine JSONL only; all child diagnostics go to stderr. Exact line forms are:

```json
{"id":"<lowercase UUID>","method":"subscribe","topic":"/atomic-swaps/2/accepts/json"}
{"id":"<lowercase UUID>","method":"publish","topic":"/atomic-swaps/2/offers/json","payload_base64":"..."}
{"id":"<lowercase UUID>","method":"stop"}
{"id":"<same UUID>","event":"ack"}
{"event":"ready","topic":"/atomic-swaps/2/accepts/json"}
{"event":"message","topic":"/atomic-swaps/2/accepts/json","payload_base64":"..."}
{"id":"<same UUID or null>","event":"error","code":"..."}
```

Unknown/duplicate keys, unsolicited IDs, partial lines, lines over 32 KiB, invalid base64, and wrong topics are rejected. At most 64 unacknowledged requests are allowed. Rust kills/restarts the child on protocol desynchronization and re-verifies every received payload.

No offer heartbeat starts until `ready` is received. Child exit or lost readiness stops new heartbeats and new candidate processing; it never enables watcher-only matching. The child environment and argv MUST contain no private key, password, mnemonic, or preimage. It receives already-signed public offer envelopes only.

The Basecamp Delivery adapter follows the same rules: subscribe globally first, forward opaque bytes to Rust, and never perform authoritative JSON filtering or verification in QJson/QML.

## Durable state and recovery

### Store guarantees

- Maker and taker each hold an exclusive OS advisory lock for the store lifetime; a second process refuses public mode.
- Files and parent directories are owner-only (`0600` files, `0700` directories).
- Every critical mutation uses write-temp, `fsync(temp)`, atomic rename, and `fsync(parent)` before returning success.
- Schema version, monotonic generation, and a checksum over serialized state are mandatory. Corrupt/unknown/newer schema fails startup; it is never reset automatically.
- Logs, receipts, UI, JSONL, and feedback never include private keys, passwords, mnemonic, raw signed transactions, or the preimage before its intentional LEZ reveal.

### Maker journal v2

Every active record contains:

```text
schema_version = 2
generation
stage
offer_envelope + offer_digest
accept_envelope + accept_digest
offer_id + eth_swap_id + eth_lock_tx_hash + hashlock
all Ethereum/LEZ identities and exact amounts/times
maker/taker ETH and LEZ identities
receipt block number/hash + transaction index + log index
reservation time + last canonical Ethereum head
LEZ create/fund transaction identifiers when known
terminal transaction/state evidence when known
last_error_code (secret-free)
```

Durable indexes independently tombstone `offerId`, `ethSwapId`, `acceptDigest`, and hashlock. Trial tombstones are retained indefinitely; storage is bounded by the one-concurrent-swap policy and operator archival.

Stages are monotonic:

```text
VerifiedReserved
-> EthRechecked
-> LezCreateIntent
-> LezCreated
-> LezFundIntent
-> Funded
-> LezClaimObserved
-> EthClaimIntent
-> EthClaimed

VerifiedReserved..Funded -> LezRefundIntent -> LezRefunded
VerifiedReserved -> RejectedReserved
any nonterminal ambiguity -> RecoveryRequired
irrecoverable partial LEZ state -> Quarantined
```

Startup reconciliation completes before Delivery readiness or offer publication. It uses only the immutable snapshot and current release allowlist, never mutable config or an ephemeral cache.

Recovery rules:

- `VerifiedReserved`/`EthRechecked`: re-run full Ethereum checks. If still safe, proceed; otherwise tombstone with zero LEZ calls.
- `LezCreateIntent`: inspect exact PDA across bounded authoritative reads. Exact escrow proceeds; authoritative absence plus a definitively rejected/dropped original transaction may retry the identical create. Ambiguity remains `RecoveryRequired` and is not rebroadcast.
- `LezCreated`/`LezFundIntent`: verify exact escrow and balance. Fund only if the original funding transaction is definitively absent/noncanonical and balance is exactly zero. Unknown or partial balance is never retried automatically.
- `Funded`: resume LEZ claim/refund observation. Exact claim preimage drives durable ETH claim retry. Exact LEZ refund terminalizes. Unknown reads retain state.
- `EthClaimIntent`: re-read Ethereum. `CLAIMED` terminalizes; `OPEN` retries identical claim with bounded backoff while possible; `REFUNDED` is a critical loss state retained for evidence.
- `Quarantined`: never retry or reuse the hashlock/secret; surface operator action.

The existing v1 `PreLock`/`Funded` entries require an explicit one-time migration. Because they lack signed v2 snapshots they MUST be reconciled under legacy restricted-counterparty rules before public mode can start; they cannot be promoted into v2.

### Taker journal v2

Taker state contains the full signed offer, preimage/hashlock, exact times, prepared raw Ethereum lock transaction and hash, computed swap ID, canonical receipt metadata, signed accept, LEZ evidence, refund/claim transactions, and stage. The preimage and raw transactions are stored only in the owner-only state file and are redacted everywhere else.

Stages are:

```text
Prepared
-> EthLockPrepared
-> EthLockBroadcast
-> EthLockedConfirmed
-> AcceptPublishing
-> LezFunded
-> LezClaimIntent
-> LezClaimed

EthLockPrepared..AcceptPublishing -> RefundWaiting
-> EthRefundPrepared -> EthRefundBroadcast -> EthRefunded
```

The Ethereum client MUST separate transaction preparation/signing from broadcast so the deterministic tx hash and raw signed transaction are durable before network submission. Recovery queries that hash and rebroadcasts the identical raw transaction only; it never creates a replacement lock with changed nonce/timelock.

On restart, publish the same accept if still fresh; otherwise resume exact LEZ observation or `RefundWaiting`. Delivery timeout is not refund availability. `RefundComplete`/`EthRefunded` requires a canonical receipt and `getHTLC == REFUNDED`; `.ok()`, submission success, pending state, timeout, and RPC outage are nonterminal. Secret-bearing state is deleted only after LEZ claim plus canonical ETH `CLAIMED`, or canonical ETH `REFUNDED` with authoritative proof that no LEZ claim obligation exists. A secret-free receipt/tombstone remains.

## Protocol error contract

Public APIs return a stable code, retry class, and secret-free message. Internal causes may be logged only after redaction.

| Code | Class | Retry | Required effect |
|---|---|---:|---|
| `PACKET_TOO_LARGE` | transport | no | drop before parse/cache |
| `INVALID_JSON` | transport | no | drop before cache |
| `INVALID_ENVELOPE` | transport | no | drop before cache |
| `UNSUPPORTED_PROTOCOL` | policy | no | no RPC/LEZ |
| `INVALID_ENCODING` | validation | no | no RPC/LEZ |
| `INVALID_SIGNATURE` | authentication | no | no cache/RPC/LEZ |
| `UNTRUSTED_MAKER` | authentication | no | disable acceptance |
| `TRUST_RECORD_EXPIRED` | authentication | after update | disable public mode |
| `OFFER_EXPIRED` | freshness | with fresh offer | no reservation |
| `ACCEPT_EXPIRED` | freshness | with new swap only | no reservation |
| `OFFER_MISMATCH` | linkage | no | no RPC/LEZ |
| `REPLAYED_OR_CONSUMED` | replay | no | preserve tombstone |
| `CACHE_FULL` | backpressure | yes | reject new work, unhealthy |
| `CHAIN_ID_MISMATCH` | chain | no | stop public mode |
| `CONTRACT_CODE_MISMATCH` | chain | no | stop public mode |
| `ETH_RPC_UNAVAILABLE` | chain | yes | fail closed, no new LEZ |
| `ETH_RECEIPT_INVALID` | chain | no | tombstone if reserved |
| `ETH_NOT_CONFIRMED` | chain | yes | wait, no LEZ |
| `ETH_REORG` | chain | yes/recovery | no new LEZ; retain active record |
| `ETH_HTLC_MISMATCH` | chain | no | no LEZ |
| `ETH_HTLC_NOT_OPEN` | chain | no/recovery | no new LEZ |
| `UNSAFE_TIMELOCK` | policy | no | no LEZ |
| `INSUFFICIENT_HEADROOM` | policy | no | no LEZ |
| `LEZ_IDENTITY_UNAVAILABLE` | chain | yes | stop public mode |
| `LEZ_ESCROW_MISMATCH` | chain | no | no reveal/funding |
| `LEZ_STATE_UNKNOWN` | chain | yes/recovery | retain durable state |
| `INSUFFICIENT_INVENTORY` | policy | after funding | stop offers |
| `RESERVATION_CONFLICT` | concurrency | no | deterministic loser, no LEZ |
| `JOURNAL_IO` | durability | after repair | zero LEZ before durable record |
| `JOURNAL_CORRUPT` | durability | operator | stop public mode; never reset |
| `SIDECAR_PROTOCOL` | transport | after restart | stop heartbeats/new accepts |
| `REFUND_PENDING` | recovery | yes | never report completion |
| `RECOVERY_REQUIRED` | recovery | yes/operator | retain record; no duplicate action |
| `QUARANTINED` | recovery | operator | never reuse hashlock |

Every result exposed to C++/QML uses decimal strings and canonical identifiers. Error details MUST NOT echo raw packets or secrets.

## Golden EIP-712 vectors

These vectors use the release-current Sepolia chain, contract, and runtime-code pins. All actor identities, LEZ identities, messages, and private keys remain synthetic nondeployment values. The two private keys are public test keys and MUST never be used for funds.

```text
maker private key = 0x59c6995e998f97a5a0044976f7d13e8e7f6fbba83ce7f0b1037f6142155b28f0
maker address     = 0x4deef74c0c46e1267a126b16af3a7c151b3c6c85
taker private key = 0x8b3a350cf5c34c9194ca3a545d65b8a31b5079b7214c06e24b7e0821a30bd55d
taker address     = 0x4e4cdb4676d22a569ff136ff79dcdff5d1766734
```

Domain/vector constants:

```text
ethereumChainId       = 11155111
ethereumHtlc          = 0x351b0ea07739fa9f6769213927d7836a790a5faf
lezChainId            = 0x2222222222222222222222222222222222222222222222222222222222222222
lezHtlcProgramId      = 0x3333333333333333333333333333333333333333333333333333333333333333
ethereumHtlcCodeHash  = 0xbad9367560aa868d44420e15b958ad1c5644cdd20ef4bed85af4d1c33d3fa1a2
makerLezAccount bytes = 0x5555555555555555555555555555555555555555555555555555555555555555
makerLezAccount JSON  = 6k78AbasGMFFrhG95Pj6jQbqkVt7FQMhVgemxJovWKR6
takerLezAccount bytes = 0x9999999999999999999999999999999999999999999999999999999999999999
takerLezAccount JSON  = BLbDu5FZUdSfLrGejhuaWw5iMJBo3j3TVRyPv9rfJyMA
domain type hash      = 0xd87cd6ef79d4e2b95e15ce8abf732db51ec771f1ca2edccf22a46c729ac56472
domain salt           = 0xa068a79e8729e9a3722281426753c1ad84cd1c749aa83c9e08be6f541c0df9a7
domain separator      = 0xc520ac51bfbdde77a788eb01b046af1028d343084a932896f5fbab9538a5f906
```

Offer typed values:

```json
{
  "offer_id":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "ethereum_chain_id":"11155111",
  "ethereum_htlc":"0x351b0ea07739fa9f6769213927d7836a790a5faf",
  "ethereum_htlc_code_hash":"0xbad9367560aa868d44420e15b958ad1c5644cdd20ef4bed85af4d1c33d3fa1a2",
  "lez_chain_id":"0x2222222222222222222222222222222222222222222222222222222222222222",
  "lez_htlc_program_id":"0x3333333333333333333333333333333333333333333333333333333333333333",
  "maker_eth_address":"0x4deef74c0c46e1267a126b16af3a7c151b3c6c85",
  "maker_lez_account":"6k78AbasGMFFrhG95Pj6jQbqkVt7FQMhVgemxJovWKR6",
  "eth_amount_wei":"100000000000000",
  "lez_amount":"150",
  "lez_timelock_duration_sec":"1200",
  "eth_timelock_duration_sec":"2400",
  "min_timelock_margin_sec":"300",
  "max_fills":"1",
  "issued_at":"2000000000",
  "expires_at":"2000000120"
}
```

Expected offer results:

```text
offer type hash   = 0x8336c1e25297cee0ed720120007651c362617dc754421b905c6d24ad473f0e6f
offer struct hash = 0x3c90b0b5f24883e0b933405a1bab7248d58bff8d117b54d874c53e3a532a54a9
offer digest      = 0x81773d63672ebc0d0338213cda7b81e880f6b6b92950462adbcf277b9c501f3a
offer signature   = 0x657314eb0442a7b4b4050756e8ab806be801b44043c7b0af99148778c63bb4022f73783c4abfc60135c95a5ca195e7ca67138629d4775bd1cf604a6003297c6b1b
recovered signer  = 0x4deef74c0c46e1267a126b16af3a7c151b3c6c85
```

Accept-specific values:

```text
hashlock         = 0x8888888888888888888888888888888888888888888888888888888888888888
ethRefundAfter   = 2000002400
lezRefundAfter   = 2000001200000
ethSwapId        = keccak256(abi.encodePacked(
                     takerAddress,
                     makerAddress,
                     uint256(100000000000000),
                     hashlock,
                     uint256(2000002400),
                     takerLezAccount
                   ))
                 = 0x31d9f1691a1533b8bb69de5f466fc65e5e28733260cd721dfbdb266d0e6ff039
ethLockTxHash    = 0x7777777777777777777777777777777777777777777777777777777777777777
issuedAt         = 2000000000
expiresAt        = 2000000180
```

Accept typed values use the offer values above plus:

```json
{
  "offer_id":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "offer_digest":"0x81773d63672ebc0d0338213cda7b81e880f6b6b92950462adbcf277b9c501f3a",
  "ethereum_chain_id":"11155111",
  "ethereum_htlc":"0x351b0ea07739fa9f6769213927d7836a790a5faf",
  "ethereum_htlc_code_hash":"0xbad9367560aa868d44420e15b958ad1c5644cdd20ef4bed85af4d1c33d3fa1a2",
  "lez_chain_id":"0x2222222222222222222222222222222222222222222222222222222222222222",
  "lez_htlc_program_id":"0x3333333333333333333333333333333333333333333333333333333333333333",
  "maker_eth_address":"0x4deef74c0c46e1267a126b16af3a7c151b3c6c85",
  "maker_lez_account":"6k78AbasGMFFrhG95Pj6jQbqkVt7FQMhVgemxJovWKR6",
  "eth_amount_wei":"100000000000000",
  "lez_amount":"150",
  "eth_swap_id":"0x31d9f1691a1533b8bb69de5f466fc65e5e28733260cd721dfbdb266d0e6ff039",
  "eth_lock_tx_hash":"0x7777777777777777777777777777777777777777777777777777777777777777",
  "hashlock":"0x8888888888888888888888888888888888888888888888888888888888888888",
  "taker_eth_address":"0x4e4cdb4676d22a569ff136ff79dcdff5d1766734",
  "taker_lez_account":"BLbDu5FZUdSfLrGejhuaWw5iMJBo3j3TVRyPv9rfJyMA",
  "eth_refund_after":"2000002400",
  "lez_refund_after":"2000001200000",
  "issued_at":"2000000000",
  "expires_at":"2000000180"
}
```

Expected accept results:

```text
accept type hash   = 0x8ceda785542cc8cff7ebbb8d2dadc525c1b45249f0833a8dc2d839dc7d28dcad
accept struct hash = 0xc4547967954a25efbba4fee4a6a8b5ed7c4f68474f622053c7562c897d358f19
accept digest      = 0x2c49d007e29f513b1b8c86f4d10d064a6fcfeaedb0a31dab77665091c0d7e721
accept signature   = 0x5a4989399efa4ff493f80399970b0218a5e796edd64414b8c2f5b156e863d9183538b7f04232461691ed38431c84930d2c1ea3afb992ebfaf82b52304b80feb31c
recovered signer   = 0x4e4cdb4676d22a569ff136ff79dcdff5d1766734
```

The accept vector above uses the normative millisecond `lezRefundAfter` and exact 180-second accept TTL. Implementations MUST reproduce these expected hashes/signatures with both the Rust implementation and an independent tool before merge.

### Mutation vectors

For both messages, flip one bit or choose a distinct valid value in every field, recompute neither signature, and require signature/linkage rejection. Independently cover wrong domain name/version/chain/contract/salt/type, high-`s`, `v` 0/1/29, 64-byte compact signature, zero `r/s`, duplicate/unknown JSON key, number instead of string, base58 alternate, uppercase hex, overflows, and values over `2^53`. Every case asserts zero RPC for pre-RPC failures and zero LEZ calls for all failures.

## Required test and merge gates

1. Commit the exact type strings and the full vectors above as shared Rust fixtures. `cast` or ethers must independently reproduce both signatures and recovered addresses.
2. Test every-field mutations, domain separation, malformed signatures, and strict JSON lexical/size/depth bounds.
3. Test self-signed but unpinned offers, expired trust records, a rotated signer, and an update containing no active trust record.
4. Prove two accepts for one offer, one swap under two offers, duplicate after restart, and same hashlock/new swap produce one durable winner and at most one LEZ funding.
5. Simulate 0/1/2/3 confirmations, receipt disappearance, block hash replacement before each recheck, RPC disagreement, code change, log duplication, and non-OPEN state.
6. Crash at every maker and taker durable boundary, including file/directory fsync failure. Restart must reconcile without a second funding and without false refund completion.
7. Exercise the active Delivery runtime and Node JSONL child with drop, reorder, duplicates, floods, partial/oversized lines, stdout log injection, exit/restart, and pipe backpressure.
8. Test exact LEZ decoder/owner/PDA/account/amount/time/state/balance mismatches and malformed/truncated/oversized escrow data without panic across FFI.
9. Run genuine Sepolia↔LEZ Claim and Refund paths. Record receipt/block/log evidence and prove 3-confirmation checks and exact escrow checks in logs without secrets.
10. An independent security reviewer must trace all public-mode call paths and prove an invalid/missing/expired accept cannot reach any LEZ create/fund call.

## Remaining blockers

These are the remaining protocol, integration, external-value, and approval gates:

1. **BLOCKED — authoritative LEZ chain identity:** LEZ must expose the stable 32-byte `get_network_identity` capability above, and two independent endpoints must return the same release-pinned value.
2. **BLOCKED — maker trust deployment:** choose the actual maker EOA/LEZ account, publish the exact `TrustedMakerV2` record and seven-day validity/renewal process, and approve package-based rotation/revocation.
3. **BLOCKED — contract provenance:** the runtime hash agrees across two independent Sepolia providers and the repository carries the deployment record/source pointer, but an independent provenance review still must reproduce the reviewed build and link its runtime bytecode to that deployment.
4. **BLOCKED — shared typed fixtures and mutation coverage:** an independent reviewer reproduced the corrected interface-v2 deployment values, six-field swap ID, EIP-712 hashes, signatures, recovered EOAs, and both base58 account decodings. The executable fixture gate now enforces the duplicated typed fields and account encodings, but merge still requires a shared Rust consumer plus the full mutation suite above.
5. **BLOCKED — taker LEZ proof of possession:** `SwapAcceptV2` currently proves only the Ethereum EOA and accepts a self-declared LEZ account. Specify and independently review a LEZ-native proof bound to the exact accept/offer digest, then verify it before reservation or any LEZ call. Without it, a cheap attacker can monopolize the one-fill maker by repeatedly locking inventory to third-party accounts nobody can claim.
6. **BLOCKED — shared Basecamp wallet ownership:** the current app still owns raw LEZ key/wallet-path configuration. Public Basecamp mode must consume the released shared `lez_core` account/signing boundary and expose no raw signing key, wallet path, or fallback secret-bearing configuration.
7. **BLOCKED — identity-linkability and retention disclosure:** the protocol links the taker Ethereum address/transaction/hashlock to `takerLezAccount` on a public chain and retains durable replay tombstones. Approve the minimum retention policy and require an explicit tester-facing disclosure before acceptance.
8. **BLOCKED — LEZ submission/recovery API:** split create/fund submission or expose deterministic transaction identity/status so ambiguous broadcasts cannot be automatically duplicated.
9. **BLOCKED — durable taker integration:** the current client returns `{swap_id, tx_hash}` only after receipt and keeps preimage/progress in memory. It must prepare and persist the raw signed transaction, transaction hash, swap ID, receipt metadata, and journal stages above before broadcast/progress reporting.
10. **BLOCKED — Rust authorization ownership:** current C++/Delivery and watcher path is not the Rust-owned v2 gate. No public implementation is approved until the single entry point and no-fallback proof exist.
11. **BLOCKED — operations approval:** approve 600-second LEZ headroom, 3 Sepolia confirmations, 256-block replay, rate limits, and the seven-day trust-pin lifetime for the public trial.

When all eleven items are cleared and the required test matrix passes, the decision changes to **GO for public-mode implementation and release testing**. Until then the only safe result is **NO-GO**.

## Current-code deltas implied by this contract

- `src/swap/maker.rs`: replace event-authorized matching, `amount >=`, fail-open `get_htlc`, static taker, and narrow journal trait with typed v2 authorization/snapshot APIs.
- `src/swap/taker.rs`: exact LEZ equality, durable preimage/accept/refund state, prepared Ethereum transaction flow, and confirmed refund terminality.
- `src/eth/client.rs`: return `{swap_id, tx_hash, receipt metadata}`, expose chain/code/receipt/block reads, prepare-before-broadcast, and canonical state helpers.
- `src/eth/watcher.rs`: confirmation/reorg-aware hints; never an authorization source.
- `src/cli/bot.rs` and `src/cli/maker.rs`: journal v2, exclusive process lock, reconciliation, Rust signing, bidirectional transport-only child, and subscription-before-advertise.
- `swap-module/src/swap_delivery_adapter.*`: opaque bounded v2 forwarding and global accept subscription; remove QJson field filtering from the security path.
- LEZ host decoder/client: total decode, exact owner/PDA/state validation, network identity, and split/recoverable create/fund submission.
- Tests: shared golden vectors plus negative, finality, concurrency, crash, sidecar, and real-runtime coverage listed above.
