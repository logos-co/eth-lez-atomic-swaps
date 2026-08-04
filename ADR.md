# Atomic Swaps — Architecture Decision Records

## ADR (v0.3)

### Decisions

1. **Product interface**: The `swap` and `swap_ui` modules running inside Logos Basecamp are the only supported desktop UI. The repository does not ship or test a standalone app host. `swap-cli` remains supported as headless operator, automation, and protocol-regression tooling; it is not the end-user app
2. **UI acceptance host**: Runtime UI acceptance must install the portable LGX packages and exercise them inside the pinned Basecamp build. A module build or headless CLI swap is useful supporting evidence, but does not prove the Basecamp journey

---

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
4. **Interface (superseded by v0.3)**: New standalone CLI — will try using Logos Core, but not make it a blocking dependency. The CLI is now retained only for headless operator, automation, and protocol-regression use; Basecamp modules are the product interface
5. **Counterparty negotiation**: Hardcoded swap params for PoC; discovery via Logos Messaging deferred to a later phase
