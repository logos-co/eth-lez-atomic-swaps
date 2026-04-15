# AGENTS.md

This repository is a proof-of-concept for cross-chain atomic swaps between LEZ and Ethereum using hash time-locked contracts (HTLCs).

Read this file before making changes. Prefer small, targeted diffs and validate only the parts of the system affected by the change.

## Terminology

- `LEZ` is the Logos Execution Zone, the execution environment this repo targets on the Logos side.
- `LSSA` is the upstream chain/runtime stack and crate family this repo integrates with through crates like `nssa`, `nssa_core`, `wallet`, and `sequencer_service_*`.
- In this repo, treat `LEZ` as the product/runtime name and `LSSA` as the underlying upstream implementation surface unless the code or external docs require more specific wording.
- Be precise when editing docs and user-facing copy: do not casually rename one to the other.

## Project shape

- `src/`: Rust orchestration library and CLI entrypoints.
- `tests/`: Rust integration and end-to-end tests.
- `contracts/`: Solidity Ethereum HTLC contract and Foundry tests.
- `programs/`: LEZ HTLC program and generated methods crates.
- `swap-ffi/`: Rust FFI bridge used by the Qt UIs.
- `ui/`: standalone Qt UI.
- `logos-module/`: Logos app module / plugin integration.

## Ecosystem relationships

- `logos-scaffold` is the current LEZ/LSSA developer entrypoint for this repo. Upstream, it is a Rust CLI for bootstrapping standalone LSSA `program_deployment` projects, managing localnet, wallet state, and deploy/build flows.
- `scaffold.toml` pins the upstream `lssa` repo and commit for this project. Respect that pin unless the task is explicitly about upgrading or re-pinning upstream.
- `spel` is the LEZ program framework and CLI layer, roughly analogous to an Anchor-style developer framework. It is relevant when touching program/IDL/CLI ergonomics, even if this repo does not directly route everything through `spel` yet.
- `logos-module-builder` is upstream infrastructure for reducing Logos module boilerplate and standardizing module packaging. Upstream currently labels it experimental. Treat it as an emerging target direction, not as a stable assumption already adopted here.
- This repo sits in the middle of those moving pieces. Changes here can expose missing seams between app/plugin development, LEZ program workflows, and Scaffold-driven local development.

## Core domain rules

- This repo implements a taker-locks-first atomic swap flow.
- Use SHA-256 for the hashlock. Do not silently switch to keccak or another hash.
- ETH uses the longer timelock; LEZ uses the shorter timelock.
- LEZ timelock enforcement is on-chain via `ValidityWindow`; off-chain checks are only UX safeguards.
- Messaging via nwaku is optional. The swap flow should not assume messaging is always available.
- Treat this as a PoC: preserve clarity and correctness over abstraction churn.

## Environment and dependencies

- Rust toolchain is pinned in `rust-toolchain.toml` to `1.93.0`.
- Main prerequisites: Rust, Foundry, Docker, `logos-scaffold`, CMake, and Qt 6.5+.
- Many Rust tests and demo flows expect `NSSA_WALLET_HOME_DIR=.scaffold/wallet`.
- Local infrastructure can start external services and write `.env` files. Avoid running those commands unless the task actually needs them.
- `logos-scaffold setup` and related flows are not incidental helpers. They are part of the intended developer path for LEZ/LSSA work in this repo.
- The current repo-level LSSA pin lives in `scaffold.toml`; Cargo dependencies and local behavior should be interpreted in that context.

## Files to treat carefully

- Do not overwrite `.env`, `.env.taker`, or `.env.example` unless the task is explicitly about configuration.
- Do not edit generated or vendored dependencies unless required:
- `target/`
- `contracts/lib/`
- `.scaffold/`
- lockfiles should change only when dependency work requires it.
- The repo may contain user work in progress. Never revert unrelated changes.

## Preferred workflow

1. Read the smallest relevant set of files first.
2. Describe the likely impact before making broad changes.
3. Make the minimum viable code change.
4. Run the narrowest validation that gives confidence.
5. Capture any upstream friction or missing-tooling observations while they are fresh.
6. Summarize what changed, what was validated, what remains unverified, and any dogfooding feedback surfaced by the work.

## Validation commands

Prefer the cheapest command that matches the edited surface area.

### Rust orchestration / CLI

- `cargo check`
- `cargo test --test <name>`
- `cargo test <filter>`
- `NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo test`

Use the full `NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo test` path only when the change touches LEZ integration, orchestration flows, or end-to-end behavior.

### Solidity contracts

- `cd contracts && forge build`
- `cd contracts && forge test`
- `cd contracts && forge fmt`

If a change affects the Ethereum HTLC contract or its ABI, validate inside `contracts/` with Foundry.

### Messaging

- `make nwaku`
- `cargo test -- --ignored`

Only run ignored messaging tests when the task touches nwaku messaging behavior.

### Full-stack / infra-heavy

- `make contracts`
- `make test`
- `make demo`
- `make infra`

These are expensive and may start Docker, Anvil, LEZ localnet, and nwaku. Use them intentionally, not by default.

## UI and plugin notes

- `make configure` and `make build` build the standalone Qt UI.
- `make plugin-build` builds the Logos app plugin variant.
- Qt / CMake changes should avoid unnecessary churn in unrelated UI files.
- On macOS, plugin install/build logic may reference local Nix store Qt paths and local Logos app paths from the `Makefile`. Do not generalize those paths without checking whether the task requires it.
- When touching module/plugin packaging, think beyond this repo's current Makefile. The longer-term direction is a smoother Logos developer journey converging Scaffold, SPEL, and module-building workflows rather than keeping them as permanently separate tracks.

## Change heuristics

- Prefer preserving current module boundaries unless the existing structure is actively blocking the fix.
- For swap logic changes, inspect both maker and taker flows before editing one side in isolation.
- For config changes, review `src/config.rs`, CLI parsing, `.env.example`, and any code that writes env files.
- For infra changes, review `src/scaffold.rs`, `src/cli/infra.rs`, `Makefile`, and Docker/localnet assumptions together.
- For contract interface changes, review Rust callers and tests that depend on generated ABI artifacts.
- For LEZ-side program or wallet flow changes, check whether the right fix belongs in this repo, in Scaffold, in SPEL, or upstream in LSSA rather than patching around the problem locally.
- Prefer fixes that clarify the intended developer path. Avoid papering over rough edges with repo-local hacks if the issue is clearly an upstream workflow problem.
- If you need a local workaround because upstream is still moving, keep it explicit and document the upstream issue it compensates for.

## Upstream movement and dogfooding

- Upstream LEZ/LSSA, Scaffold, SPEL, and module tooling are evolving quickly. Assume some friction may come from ecosystem churn rather than only from local code defects.
- When implementation work reveals unclear APIs, awkward setup, mismatched assumptions, or broken workflow boundaries, record that as dogfooding feedback.
- Separate these categories in your reasoning and summaries:
- local bug in this repo
- repo integration gap
- upstream bug or regression
- upstream product/design gap in the Logos developer journey
- Good changes in this repo should not only make the PoC work, but also clarify what the upstream platform should improve to streamline building on Logos.

## What to report back

- Files changed.
- Commands run.
- Whether validation was full, partial, or skipped.
- Any infra prerequisites the user must have running to reproduce results.
- Any dogfooding feedback or upstream follow-up uncovered during the task.
