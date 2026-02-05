 Atomic Swaps HTLC PoC — Task Breakdown                                       │
│                                                                              │
│ Standalone Rust binary. Maker sells λ (LEZ) for ETH (Ethereum). Hardcoded    │
│ swap params. Throwaway PoC.                                                  │
│                                                                              │
│ ---                                                                          │
│ Tasks                                                                        │
│                                                                              │
│ 1. Ethereum HTLC Smart Contract                                              │
│                                                                              │
│ - Solidity HTLC contract: lock(hashlock, timelock, recipient),               │
│ claim(preimage), refund(). Events: Locked, Claimed, Refunded                 │
│ - Foundry unit tests (happy path, timeout refund, failure cases)             │
│ - Sepolia deploy script                                                      │
│                                                                              │
│ 2. LEZ HTLC Program (Rust / Risc0 zkVM)                                      │
│                                                                              │
│ - HTLC program built with nssa_core SDK. Escrow via PDA storing hashlock,    │
│ maker/taker IDs, amount, state                                               │
│ - Instructions: Lock (Maker deposits λ), Claim (Taker reveals preimage,      │
│ SHA-256 verified via risc0_zkvm::sha), Refund (authorized Maker reclaims,    │
│ off-chain timelock enforcement)                                              │
│ - Unit tests (happy path, wrong preimage, unauthorized refund)               │
│ - Compile via cargo risczero build, deploy via wallet deploy-program         │
│ - Reference: examples/program_deployment/ and programs/ in                   │
│ https://github.com/logos-blockchain/lssa                                     │
│                                                                              │
│ 3. Logos Core CLI Experiment (timeboxed 1-2 days)                            │
│                                                                              │
│ - Evaluate feasibility of using Logos Core standalone app framework /        │
│ https://github.com/logos-blockchain/logos-blockchain-module as CLI for the   │
│ swap                                                                         │
│ - Document findings — proceed with standalone binary regardless              │
│                                                                              │
│ 4. Standalone Rust CLI Binary                                                │
│                                                                              │
│ 4a. Project setup                                                            │
│                                                                              │
│ - Cargo workspace, dependencies: alloy (Ethereum), nssa_core + wallet crate  │
│ (LEZ), tokio, clap                                                           │
│ - Config: RPC endpoints, wallet keys, contract addresses via env vars        │
│                                                                              │
│ 4b. Maker flow                                                               │
│                                                                              │
│ - Generate secret → compute SHA-256 hash                                     │
│ - Lock λ on LEZ (create escrow PDA with hashlock + taker ID)                 │
│ - Wait for Taker's ETH lock on Ethereum (uses 5a)                            │
│ - Claim ETH on Ethereum by revealing preimage                                │
│                                                                              │
│ 4c. Taker flow                                                               │
│                                                                              │
│ - Read swap offer (hardcoded: hash, amount, maker LEZ account, Ethereum HTLC │
│  address)                                                                    │
│ - Lock ETH on Ethereum with hashlock                                         │
│ - Wait for Maker's λ lock on LEZ (uses 5b)                                   │
│ - Claim λ on LEZ by revealing preimage                                       │
│                                                                              │
│ 4d. Refund flow                                                              │
│                                                                              │
│ - Maker: reclaim λ from LEZ escrow (CLI checks timelock before submitting)   │
│ - Taker: reclaim ETH from Ethereum HTLC (on-chain timelock enforced by       │
│ contract)                                                                    │
│                                                                              │
│ 5. Chain Monitoring                                                          │
│                                                                              │
│ 5a. Ethereum event watcher                                                   │
│                                                                              │
│ - Poll HTLC contract events via alloy: Locked, Claimed, Refunded             │
│ - Extract preimage from Claimed events                                       │
│                                                                              │
│ 5b. LEZ state watcher                                                        │
│                                                                              │
│ - Poll escrow PDA account state via sequencer API / wallet account get       │
│ - Detect state transitions + extract preimage from Claimed state             │
│                                                                              │
│ How 4 and 5 work together: Task 5 builds reusable polling functions          │
│ (Ethereum watcher, LEZ watcher) that Task 4 calls inline during the "wait"   │
│ steps of the maker/taker flows. The CLI runs as a single long-running        │
│ command — it submits a tx, then calls the appropriate watcher in a polling   │
│ loop until the counterparty's action is detected, then auto-progresses to    │
│ the next step. Task 5 is not a separate daemon; it provides the building     │
│ blocks that Task 4 orchestrates.                                             │
│                                                                              │
│ 6. Integration Testing                                                       │
│                                                                              │
│ - Happy path: full Maker ↔ Taker swap end-to-end (Sepolia + LEZ testnet)     │
│ - Timeout/refund: one party disappears, both reclaim funds                   │
│ - Failure cases: wrong preimage, unauthorized refund                         │
│                                                                              │
│ ---                                                                          │
│ Design Decision: Timelock on LEZ                                             │
│                                                                              │
│ Confirmed: LSSA programs receive only account pre-states and instruction     │
│ data — no block height, no timestamp. Verified on both main and              │
│ schouhy/full-bedrock-integration branches. The bedrock integration is        │
│ infrastructure plumbing (block settlement/finality on L1) and does not       │
│ expose chain metadata to programs.                                           │
│                                                                              │
│ Decision for PoC: Off-chain enforcement. The CLI checks current time before  │
│ submitting a refund tx. The HTLC program on LEZ allows refund                │
│ unconditionally when called by the authorized depositor (Maker). Acceptable  │
│ since this is a throwaway PoC with hardcoded params between cooperating      │
│ parties.                                                                     │
│                                                                              │
│ Action item: Ask LSSA team about adding block_timestamp / block_height to    │
│ ProgramInput. If supported, upgrade to on-chain enforcement in a future      │
│ iteration.                                                                   │
│                                                                              │
│ ---                                                                          │
│ Suggested Order                                                              │
│                                                                              │
│ 1 (Eth HTLC) ───────┐                                                        │
│                      ├──→ 4a (project setup) → 5 (watchers) → 4b-d (CLI      │
│ flows) → 6 (integration tests)                                               │
│ 2 (LEZ HTLC) ───────┘                                                        │
│ 3 (Logos Core experiment) — independent, can run anytime                     │
│                                                                              │
│ - Tasks 1 and 2 can run in parallel (no dependency between Eth and LEZ       │
│ contracts)                                                                   │
│ - Task 3 is independent and timeboxed                                        │
│ - Task 4a can start once either 1 or 2 is done                               │
│ - Task 5 depends on 1 and 2 (needs contract ABIs / account structures to     │
│ poll)                                                                        │
│ - Tasks 4b-d depend on 5 (CLI flows call the watchers during "wait" steps)   │
│ - Task 6 depends on everything above
