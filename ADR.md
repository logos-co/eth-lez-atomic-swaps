# Atomic Swaps — Architecture Decision Records

## ADR (v0.2)

### Decisions

1. **Locking order**: Taker locks first (ETH, longer timelock), maker locks second (LEZ, shorter timelock). If maker locked first, a malicious taker could repeatedly initiate swaps without completing them, timelocking all of the maker's funds until expiry
2. **Preimage ownership**: Taker generates and holds the preimage; maker only receives the hashlock
3. **Account ID format**: Base58 for display and storage, matching wallet CLI convention
4. **Scaffold integration**: Use logos-scaffold as the one-stop shop for Logos app development

---

## ADR (v0.1)

### Decisions

1. **Target chain**: Ethereum  - familiarity, simpler see HTLC, potential usage of eth wallet module, top 3 desired from strategy
2. **Swap direction**: Maker sells λ (LEZ) for ETH — prioritises bootstrapping inbound liquidity to LEZ
3. **Swap mechanism**: HTLC — simplest trust-minimised primitive; adaptor signatures deferred to a later phase
4. **Interface**: New standalone CLI — will try using Logos Core, but not make it a blocking dependency
5. **Counterparty negotiation**: Hardcoded swap params for PoC; discovery via Logos Messaging deferred to a later phase
