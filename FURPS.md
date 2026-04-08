# Atomic Swaps — FURPS+

## FURPS+ (v0.2)

Phase 2: Taker-Locks-First, Balance Visibility, Maker Automation, Scaffold Integration, Chat-Based Negotiation

- **Maker**: the party that publishes swap offers and sells λ for ETH
- **Taker**: the party that accepts and initiates the swap by locking ETH first

### Functionality

1. Taker locks first: taker generates secret preimage, locks ETH (longer timelock), maker locks LEZ (shorter timelock), taker claims LEZ (reveals preimage), maker learns preimage and claims ETH
2. Wallet balance display: both ETH and LEZ balances visible in UI before and after swaps
3. Maker auto-accept: start/stop toggle that broadcasts offers and auto-executes swaps until it runs out of funds
4. Integrate Delivery Logos Module (messaging) instead of using the REST API
5. Maker sells λ for ETH (unchanged - v0.1)
6. Original owners can reclaim funds after timeout (unchanged — v0.1)
7. Native tokens only: ETH, λ (unchanged — v0.1)

### Usability

1. Separate maker/taker interfaces: each role sees only its relevant controls
2. UI displays wallet balances
3. Account IDs in base58 format, consistent with wallet CLI
4. Swap orchestration library monitors both chains for swap events and preimage reveals (unchanged — v0.1)
5. Clear error messages on failure, timeout, or invalid state transitions (unchanged — v0.1)
6. Default timeouts for demo: 5-10 minutes (unchanged — v0.1)

### Reliability

1. Auto-accept maker loop resilience: on swap failure, log error and continue to next offer (don't crash)
2. Refund path for incomplete swaps available for both parties (unchanged — v0.1)

### Performance

### Supportability

1. Logos-scaffold/SPEL integration: replace temp-directory wallet management with scaffold-managed persistent state
2. Swap orchestration library (Rust) is interface-agnostic (unchanged — v0.1)

### + (Privacy, Anonymity, Censorship-Resistance)

1. On-chain traces of atomic swaps on Ethereum chain (unchanged — v0.1)
2. Cross-chain linkability via shared hashlock (unchanged — v0.1)
3. Amounts visible on ETH side (unchanged — v0.1)

### Dependencies (v0.2)

#### LEZ

- Validity Windows not available — continue with off-chain timelock enforcement

#### Logos Scaffold

- Wallet state management and sequencer management via scaffold

#### Chat Module

- Integrate Delivery Logos Module (messaging) instead of using the REST API
- Offer broadcast and discovery via Delivery Logos Module (messaging)
- Chat module enables bootstrapping a conversation based on information maker broadcasts

---

## FURPS+ (v0.1)

[v0.1 milestone](https://github.com/logos-co/ecosystem/milestone/7)

Phase 1: HTLC PoC — Maker sells λ (LEZ) for ETH (Ethereum)

- **Maker**: the party that creates and publishes the swap offer
- **Taker**: the party that accepts and initiates the swap

### Functionality

1. Maker sells λ for ETH (Ethereum)
2. Original owners can reclaim funds after timeout if swap did not complete
3. Native tokens only (ETH on Ethereum, λ on LEZ)

### Usability

1. Daemon monitors both chains for swap events and preimage hash reveals
2. Clear error messages on failure, timeout, or invalid state transitions
3. Default timeouts for demo purposes (5-10 min)
4. User may need to run specific CLI commands to progress swap; Or a daemon will be available (TBC)

### Reliability

1. Refund path for incomplete swaps is available for both parties

### Performance

### Supportability

1. New CLI

### + (Privacy, Anonymity, Censorship-Resistance)

1. On-chain traces of atomic swaps on Ethereum chain
2. Cross-chain linkability via shared hash lock: the two sides of a swap are correlatable on-chain regardless of account privacy on LEZ. Private LEZ accounts hide participant identity but not the swap linkage itself
3. Amounts visible on ETH side

### Dependencies (v0.1)

#### LEE / LEZ Wallet Module

- Validity Windows (LEE block context) LEZ programs need a way to enforce timeouts. Validity windows (`valid_from` / `valid_until`) let the swap contract distinguish "before deadline" from "after deadline" without leaking timing metadata
- Watching events/equivalent

#### Ethereum Wallet Module

- Deploying smart contract
- Getting events

#### Chat Module

For v0.2:

- We will aim to have some negotiation - ideally chat module enable bootstrapping a conversation based on information maker broadcasts.
