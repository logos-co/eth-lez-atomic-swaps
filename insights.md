# LEZ / LSSA Insights

Findings discovered while building and testing the atomic swap orchestration layer.

## Account Ownership & Claiming

LSSA uses a program ownership model for accounts:

- **Uninitialized accounts** have `program_owner == DEFAULT_PROGRAM_ID` (all zeros).
- A program **claims** an account by returning `AccountPostState::new_claimed(account)`. The sequencer then sets `program_owner` to the executing program's ID.
- A program can **only claim accounts that currently have `DEFAULT_PROGRAM_ID`** as their owner. Attempting to claim an already-owned account returns `InvalidProgramBehavior`.
- **Only the owning program** can modify an account's data or decrease its balance.
- **Any program can credit** (increase balance of) any account, regardless of ownership.

### `validate_execution` Rules (nssa_core)

Key rules enforced by the sequencer after program execution:

1. Nonces must remain unchanged (sequencer increments them separately).
2. Programs cannot change `program_owner` in post-states (the sequencer handles it during claiming).
3. **Balance decrease** only allowed if `account.program_owner == executing_program_id`.
4. **Data changes** only allowed if the account is owned by the executing program **OR** the pre-state is `Account::default()` (completely uninitialized).
5. Total balance across all accounts must be preserved.

### `AccountPostState` API

- `new(account)` — no ownership change, just state update.
- `new_claimed(account)` — request ownership (only works on default-owner accounts).
- `new_claimed_if_default(account)` — conditionally claim only if currently unowned.

## Authenticated Transfer Program

The built-in transfer program (`Program::authenticated_transfer_program()`) has two modes:

1. **Initialize**: Single account with 0 balance — claims it under the transfer program.
2. **Transfer**: Two accounts — debits sender, credits recipient.

The transfer program **claims the recipient** if `program_owner == DEFAULT_PROGRAM_ID`. This means sending funds to an uninitialized account causes the transfer program to own it.

### Implication for HTLC

If you transfer funds to an escrow PDA *before* the HTLC program claims it, the transfer program owns the PDA and the HTLC program cannot modify its data. The solution is to **Lock first** (claims the PDA under the HTLC program), then transfer funds afterward.

## HTLC Lock Flow (Corrected)

The original two-step flow (Transfer → Lock) fails because:
1. Transfer claims the PDA under `authenticated_transfer_program`.
2. Lock instruction can't modify a PDA owned by another program.

The corrected flow:
1. **Lock** — PDA is `Account::default()`, so the HTLC program claims it and sets escrow data (balance stays 0).
2. **Transfer** — PDA is now owned by HTLC program, so the transfer program just credits balance without claiming.
3. The escrow is "locked but unfunded" briefly between steps. The maker's `lock()` method waits for both steps to confirm before returning.

## Sequencer Transaction Model

- `send_tx_public()` submits a transaction to the **mempool**. It returns success (tx_hash) even if the guest program will fail during execution.
- Actual execution happens during **block creation** (every `block_create_timeout_millis`).
- Guest program panics (e.g., wrong preimage) cause the transaction to be dropped silently — no error is returned to the submitter.
- To verify a transaction's effect, you must **poll account state** after waiting for block confirmation.

## RISC0 Dev Mode

With `RISC0_DEV_MODE=1`:
- The guest program **is executed** (state transitions are computed).
- The zkVM **proof is not generated** (execution is trusted).
- Guest program assertions (panics) do fire and cause transaction failure.
- This means integration tests correctly validate business logic without the overhead of proof generation.

## Sequencer Test Setup

To spin up a local test sequencer:

1. Start a **dummy WebSocket server** (jsonrpsee) — the sequencer requires a connected indexer.
2. Load `SequencerConfig` from `configs/test_sequencer.json`.
3. Override `home` (temp dir), `port` (0 for random), `indexer_rpc_url` (dummy WS), `initial_accounts`.
4. Call `sequencer_runner::startup_sequencer(config)`.
5. Deploy the LEZ HTLC program via `ProgramDeploymentTransaction`.
6. Wait one block (`block_create_timeout_millis` + margin) for deployment to commit.

### Test Timing

- `block_create_timeout_millis: 2000` in test config (2s blocks).
- Use `BLOCK_WAIT = Duration::from_secs(4)` between operations (2x block time for safety).
- Lock requires 2 blocks (Lock instruction + Transfer), claim/refund 1 more block.
- Typical test duration: ~6-10 seconds each.

## Nonce Handling

- `get_accounts_nonces()` returns the **committed** nonce (from the latest block).
- Sending two transactions back-to-back can cause nonce conflicts if both use the same committed nonce.
- Solution: Wait for the first transaction to be confirmed (poll state) before sending the second.

## PDA Derivation

Escrow PDA addresses are derived deterministically:
```rust
AccountId::from((&program_id, &PdaSeed::new(hashlock)))
```

The same hashlock always produces the same PDA address for a given program. Different hashlocks produce different PDAs.
