FURPS+ (v0.1)
Phase 1: HTLC PoC — Maker sells λ (LEZ) for ETH (Ethereum)

Maker: the party that creates and publishes the swap offer
Taker: the party that accepts and initiates the swap
Functionality
Maker sells λ for ETH (Ethereum)
Original owners can reclaim funds after timeout if swap did not complete
Native tokens only (ETH on Ethereum, λ on LEZ)
Usability
Daemon monitors both chains for swap events and preimage hash reveals
Clear error messages on failure, timeout, or invalid state transitions
Default timeouts for demo purposes (5-10 min)
User may need to run specific CLI commands to progress swap; Or a daemon will be available (TBC)
Reliability
Refund path for incomplete swaps is available for both parties
Performance
Supportability
New CLI
+ (Privacy, Anonymity, Censorship-Resistance)
On-chain traces of atomic swaps on Ethereum chain
Cross-chain linkability via shared hash lock: the two sides of a swap are correlatable on-chain regardless of account privacy on LEZ. Private LEZ accounts hide participant identity but not the swap linkage itself
Amounts visible on ETH side
ADR
Decisions
Target chain: Ethereum

Swap direction: Maker sells λ (LEZ) for ETH — prioritises bootstrapping inbound liquidity to LEZ

Swap mechanism: HTLC — simplest trust-minimised primitive; adaptor signatures deferred to a later phase

Interface: New standalone CLI — keeps PoC decoupled from Logos Core

Counterparty negotiation: Hardcoded swap params for PoC; discovery via Logos Messaging deferred to a later phase
