# Scaffold Phase 1 Validation

Validation notes for issue #27 Phase 1: safe scaffold-first adoption without removing the current Makefile fallback path.

## Environment

- OS: `Darwin Danishs-MacBook-Pro.local 25.4.0 Darwin Kernel Version 25.4.0: Thu Mar 19 19:30:44 PDT 2026; root:xnu-12377.101.15~1/RELEASE_ARM64_T6000 arm64`
- Branch: `phase-1-scaffold-first`
- Atomic-swaps validation commit: `23bc69b`
- `lgs --version`: `logos-scaffold 0.1.1`
- Local scaffold source SHA: `2fd29f54fc8621430ce06428d2636fcf0ac353fc`
- Note: the CLI exposes the 0.2-era `lgs run`, `lgs basecamp`, and `lgs report` surfaces despite the version string reporting `0.1.1`.

## Commands Run

| Command | Result |
|---|---|
| `cargo check --features demo --bin swap-cli` | Passed. |
| `make -n demo` | Passed dry-run; expands to `circuits`, `contracts`, then `lgs run --profile demo`. |
| `make -n test` | Passed dry-run; expands to `circuits`, `contracts`, then `lgs run --profile test`. |
| `make -n demo-makefile` | Passed dry-run; preserves the previous `NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo run --features demo -- demo` fallback. |
| `make -n test-makefile` | Passed dry-run; preserves the previous `logos-scaffold localnet start`, `cargo test`, `logos-scaffold localnet stop` fallback. |
| `make -n infra` | Passed dry-run; preserves Anvil + LEZ infra orchestration under `make infra`. |
| `lgs doctor --json` | Completed with `15` pass, `7` warn, `0` fail. Warnings are existing pin/default drift and dirty generated LEZ cache state. |
| `lgs basecamp doctor --json` | Completed with `3` pass, `1` warn, `0` fail. Warning: `delivery_module` captured `v0.1.1` differs from scaffold default `1fde1566291fe062b98255003b9166b0261c6081`. |
| `lgs basecamp modules --show` | Passed; captured `swap = path:./swap-module#lgx`, `swap_ui = path:./swap-ui#lgx`, and `delivery_module = github:logos-co/logos-delivery-module/v0.1.1#lgx`. |
| `lgs run --profile test` | Passed; scaffold ran build/localnet/topup/deploy and the `cargo test` post-deploy hook. |
| `lgs run --profile demo` | Passed; scaffold ran build/localnet/topup/deploy and the `cargo run --features demo -- demo --no-localnet` post-deploy hook. Anvil and Ethereum deployment remained app-owned in the demo hook. |
| `lgs basecamp build-portable` | Completed; long Nix output, no shell failure reported. This validates the distributed/portable module build surface without switching the app to scaffold-owned Basecamp launch. |
| `lgs report --out /var/folders/7c/ghpxb7sn4qx14mn7kkmy42cw0000gn/T/opencode/eth-lez-scaffold-report.tar.gz --tail 100` | Passed; wrote a sanitized diagnostics archive with 9 included and 6 skipped items. Inspect before sharing publicly. |
| `lgs localnet reset --dry-run` | Passed; printed planned sequencer DB/state cleanup without making changes. |
| `lgs localnet status --json` after profile runs | Stopped: `tracked_pid = null`, `listener_present = false`, `ready = false`. |
| `pgrep -fl 'anvil|sequencer_service|logos|basecamp'` after profile runs | No matching process output; no orphaned localnet, Anvil, Logos, or Basecamp process observed. |

## Notes

- The first profile smoke attempt hit a generated SPel cache origin mismatch: the cache displayed an SSH origin while `scaffold.toml` expects `https://github.com/logos-co/spel.git`. The raw local cache remote was normalized to HTTPS before rerunning profile validation.
- `lgs run` still does not own Anvil lifecycle. The demo profile uses `demo --no-localnet` so scaffold owns the LEZ run pipeline and the app owns Anvil plus Ethereum HTLC deployment.
- `make infra`, manual Basecamp startup, portable LGX build/install flow, Anvil startup, and Ethereum deployment remain in the Makefile/app path.
- PR 315 / issue 316 docs packet follow-up: if that packet describes the local quickstart, it should make the scaffold-first wrapper path primary (`make demo`, `make test`, or direct `lgs run --profile ...` after prerequisites) while keeping the Makefile fallback and explicitly saying scaffold does not own Anvil yet.
