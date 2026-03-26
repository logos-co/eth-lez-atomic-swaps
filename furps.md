# FURPS+ (v0.2)

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

## ADR (v0.2)

### Decisions

1. **Locking order**: Taker locks first (ETH, longer timelock), maker locks second (LEZ, shorter timelock). If maker locked first, a malicious taker could repeatedly initiate swaps without completing them, timelocking all of the maker's funds until expiry
2. **Preimage ownership**: Taker generates and holds the preimage; maker only receives the hashlock
3. **Account ID format**: Base58 for display and storage, matching wallet CLI convention
4. **Scaffold integration**: Use logos-scaffold as the one-stop shop for Logos app development

## Dependencies (v0.2)

### LEZ

- Validity Windows not available — continue with off-chain timelock enforcement

### Logos Scaffold

- Wallet state management and sequencer management via scaffold

### Chat Module

- Integrate Delivery Logos Module (messaging) instead of using the REST API
- Offer broadcast and discovery via Delivery Logos Module (messaging)
- Chat module enables bootstrapping a conversation based on information maker broadcasts
