# lez-mcp — LEZ as typed agent tools (MCP server)

An [MCP](https://modelcontextprotocol.io) server (stdio transport, official
[`rmcp`](https://crates.io/crates/rmcp) Rust SDK) that exposes the **LEZ public
testnet** and the deployed **Sepolia EthHTLC** as five typed tools for agents
(Claude Code, Claude Desktop, any MCP client).

It links this repo's `swap-orchestrator` library directly, so it shares the
workspace's single **LEZ v0.2.0 pin** — the exact version the public testnet
runs. When the testnet upgrades, the server and the swap app move together in
one coordinated `Cargo.toml` bump.

## Tools

| Tool | Kind | Arguments | What it does |
|---|---|---|---|
| `lez_balance` | read | `account_id?` (base58; omit for the server's own account) | Balance in LEZ base units. |
| `lez_fingerprint` | read | `sequencer_url?` | Compares the sequencer's `getProgramIds` builtin ImageIDs against the v0.2.0 ImageIDs embedded in this binary; per-program match/mismatch verdict. Without arguments it checks the configured sequencer **and refreshes the write gate**. |
| `lez_faucet_claim` | write | `account_id`, `target_balance?`, `max_claims?` | Pinata faucet claim (150 LEZ each, proof-of-work solved locally, sent natively — no wallet CLI needed). With `target_balance`, claims repeatedly until the balance reaches it, reporting each credit. Auto-initializes the account first when the server holds its signing key. |
| `lez_transfer` | write | `to`, `amount`, `confirm` (default `false`) | **Two-phase**: `confirm=false` returns a dry-run preview (balances, nonce, feeless note — nothing is sent); only `confirm=true` broadcasts, then waits for the recipient balance to move. Auto-sends `auth-transfer Initialize` for a fresh sender account. |
| `sepolia_htlc_status` | read | `swap_id_or_hashlock` (32-byte hex) | Reads the deployed EthHTLC (read-only — no ETH key). Tries the input as a swap id, then falls back to resolving it as a hashlock via a `Locked`-event scan. |

Amounts are `u128` base units passed as **strings**. All tool results carry
both structured JSON (`structuredContent`) and a text rendering.

## Startup safety: the version fingerprint gate

A client pinned to the wrong LEZ version does not error — the sequencer
**silently drops** transactions whose `program_id` it does not recognize. So at
startup the server fingerprints the configured sequencer (`getProgramIds` vs
the five embedded builtin ImageIDs: `amm`, `authenticated_transfer`, `pinata`,
`privacy_preserving_circuit`, `token`):

- **match** → all tools enabled;
- **mismatch or RPC failure** → read tools stay live, `lez_faucet_claim` and
  `lez_transfer` are hard-refused with the structured per-program diff.

Run `lez_fingerprint` (no arguments) to re-check and re-enable writes after a
sequencer recovery/upgrade-to-matching.

## Configuration (environment)

| Variable | Default | Notes |
|---|---|---|
| `LEZ_SEQUENCER_URL` | `https://testnet.lez.logos.co` | LEZ sequencer RPC. |
| `LEZ_SIGNING_KEY` | *(unset)* | 32-byte hex signing key. |
| `LEZ_SIGNING_KEY_FILE` | *(unset)* | Path to a file containing the hex key (preferred over inline env). |
| `LEZ_WALLET_HOME` + `LEZ_ACCOUNT_ID` | *(unset)* | Scaffold wallet on disk + base58 account id — keys stay in wallet files. |
| `ETH_RPC_URL` | `wss://ethereum-sepolia-rpc.publicnode.com` | Must be WebSocket. |
| `ETH_HTLC_ADDRESS` | `0x8636Fe66DFee166589a913140f14d5F57394834A` | Canonical Sepolia deployment. |
| `ETH_HTLC_FROM_BLOCK` | `11316985` | Deployment block; lower bound for hashlock scans. |

**Key material policy:** the LEZ signing key enters only via env / key file /
wallet-home — never as a tool argument (tool calls end up in MCP transcripts),
and it is never logged. Without any key the server still serves all reads
(and faucet claims to already-initialized accounts); only `lez_transfer` is
unavailable. No ETH key exists at all — the Sepolia tool is read-only.

## Build & client config

```sh
cargo build --release -p lez-mcp     # binary: target/release/lez-mcp
```

Claude Code:

```sh
claude mcp add lez -e LEZ_SIGNING_KEY_FILE=/secure/path/lez.key \
  -- /path/to/eth-lez-atomic-swaps/target/release/lez-mcp
```

or in `.mcp.json` / Claude Desktop config:

```json
{
  "mcpServers": {
    "lez": {
      "command": "/path/to/eth-lez-atomic-swaps/target/release/lez-mcp",
      "env": {
        "LEZ_SEQUENCER_URL": "https://testnet.lez.logos.co",
        "LEZ_SIGNING_KEY_FILE": "/secure/path/lez.key"
      }
    }
  }
}
```

Logging goes to stderr (`RUST_LOG` respected); stdout is the MCP channel.

## Tests

```sh
cargo test -p lez-mcp                                    # unit: schemas, PoW, fingerprint diffing
cargo test -p lez-mcp --test testnet -- --ignored        # live public-testnet reads
```

The write tools were smoke-tested against the public testnet with throwaway
accounts (auto-init + faucet claims + a confirmed two-phase transfer); see the
PR that introduced this crate for the captured evidence and tx hashes.

## Implementation notes

- **SDK choice:** the official `rmcp` Rust SDK (v3, `server` +
  `transport-io`) — it builds cleanly on the workspace's pinned Rust 1.93.0,
  so no hand-rolled JSON-RPC loop was needed.
- The faucet claim is **native**: it fetches the pinata account's
  `[difficulty, seed]` challenge, brute-forces the SHA-256 PoW off-thread, and
  submits an unsigned public transaction (`[pinata, winner]`, empty witness
  set) — mirroring `wallet pinata claim` without shelling out.
- The sequencer silently drops writes referencing never-initialized accounts;
  both write tools detect this (`Account::default()` check, as the wallet CLI
  does) and auto-initialize when they hold the account's key.
