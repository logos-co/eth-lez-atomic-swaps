# Public testnet swaps (Sepolia ↔ LEZ testnet)

This app targets **fully public infrastructure**: the public LEZ testnet plus
Ethereum Sepolia. The first end-to-end atomic swap on this stack completed on
2026-07-21 (evidence at the bottom).

## Version pin — why v0.2.0 (final)

The public testnet at `https://testnet.lez.logos.co` runs
**logos-execution-zone `v0.2.0` (final, commit `a58fbce2`)**. This was verified
by fingerprint: all five builtin program ImageIDs returned by the testnet's
`getProgramIds` RPC match the `v0.2.0` tag's checked-in program ELFs
bit-for-bit (`amm`, `authenticated_transfer`, `pinata`,
`privacy_preserving_circuit`, `token`), and do **not** match `v0.2.0-rc5`.

The pin **must track the deployed sequencer version exactly**, because builtin
program IDs are computed client-side from ELFs embedded at build time. A
client pinned to any other tag produces transfer transactions whose
`program_id` the sequencer does not recognize — they are silently dropped.
When the testnet upgrades, bump the seven `tag = "v0.2.0"` git deps in
`Cargo.toml` + one in `programs/lez-htlc/methods/guest/Cargo.toml` and rebuild
(the guest ImageID changes with the pin — redeploy the program and update
`LEZ_HTLC_PROGRAM_ID`).

The scaffold localnet toolchain (scaffold.toml `[repos.lez]`) is pinned to the
same `a58fbce2` commit, so the localnet demo and the public-testnet client run
the identical LEZ version in lockstep.

Note: no `logos-blockchain-circuits` tarball is needed for cargo builds of
this app — since rc5 the builtin ELFs are checked into the LEZ repo and
embedded at build time (`LOGOS_BLOCKCHAIN_CIRCUITS` is no longer read by the
pinned code).

## Migration summary (old pin → v0.2.0)

- Crate renames: `nssa`→`lee`, `nssa_core`→`lee_core`,
  `NSSATransaction`→`LeeTransaction`, `read_nssa_inputs`→`read_lee_inputs`,
  env `NSSA_WALLET_HOME_DIR`→`LEE_WALLET_HOME_DIR`.
- PDA derivation: `AccountId::for_public_pda(&program_id, &seed)` replaces
  `AccountId::from((&program_id, &seed))`.
- Guest ABI: `ProgramInput` gained `self_program_id`/`caller_program_id`;
  `ProgramOutput::new` takes them as its first two args.
- Builtins moved to the `programs` crate (needs `features = ["artifacts"]`);
  transfer instruction is now the typed
  `authenticated_transfer_core::Instruction::Transfer { amount }` (a bare
  `u128` is silently rejected by the sequencer).
- Wallet API: `new_init_storage` takes a password and returns
  `(WalletCore, Mnemonic)`; accounts live in the HD key chain
  (`storage().key_chain().public_account_ids()`), not in config
  `initial_accounts`.
- PQ/BIP340 key scheme: `PrivateKey`/`PublicKey`/`WitnessSet::for_message`
  keep their old shapes; address derivation and signing domains changed
  under the hood (`/LEE/v0.3/...` prefixes), all handled inside `lee`.
- ETH watcher polls `eth_getLogs` instead of `watch()`/pubsub — public RPC
  providers expire filters/subscriptions within seconds.

## Deployed artifacts

| What | Value |
|---|---|
| LEZ sequencer RPC | `https://testnet.lez.logos.co` |
| LEZ HTLC program ID (guest ImageID) | `27720b5b0345135d8e684eb172c27f5fb237548cc891a3ec889d0ed340504070` |
| LEZ HTLC deployment tx | `c1986c2af3fc007731533d958995507c8d8b1f447d5187cc1b8967ec238c7bf9` |
| EthHTLC contract (Sepolia) | `0x8636Fe66DFee166589a913140f14d5F57394834A` (minTimelockDelta=300s) |
| ETH RPC (app requires WebSocket) | `wss://ethereum-sepolia-rpc.publicnode.com` |

## Setting up a swap peer

1. **Build**: `cargo build --release --bin swap-cli` (risc0 toolchain needed
   only if you rebuild the guest / run `--features demo`).
2. **LEZ wallet** (per peer): build the `wallet` binary from
   logos-execution-zone `v0.2.0`, then:
   ```sh
   export LEE_WALLET_HOME_DIR=$PWD/my-wallet
   mkdir -p $LEE_WALLET_HOME_DIR
   cat > $LEE_WALLET_HOME_DIR/wallet_config.json <<EOF
   {"sequencer_addr":"https://testnet.lez.logos.co/","seq_poll_timeout":"12s",
    "seq_tx_poll_max_blocks":30,"seq_poll_max_retries":5,
    "seq_block_poll_max_amount":100}
   EOF
   echo "" | wallet account new public --label me     # prints account id + pk
   wallet auth-transfer init --account-id Public/<id>  # initialize on-chain
   wallet pinata claim --to me                         # faucet: 150 LEZ/claim, repeatable
   ```
   The maker needs ≥ `LEZ_AMOUNT`; the taker only needs the initialized
   account (LEZ txs are feeless).
3. **ETH keys**: any funded Sepolia key per peer (~0.005 ETH covers gas).
4. **Env files**: one per role — see the template below. Run:
   ```sh
   swap-cli --env-file maker.env maker     # start first (watches for the lock)
   swap-cli --env-file taker.env taker
   ```

### Env template

```sh
ETH_RPC_URL=wss://ethereum-sepolia-rpc.publicnode.com
ETH_PRIVATE_KEY=<hex, no 0x>
ETH_HTLC_ADDRESS=0x8636Fe66DFee166589a913140f14d5F57394834A
LEZ_SEQUENCER_URL=https://testnet.lez.logos.co
LEZ_WALLET_HOME=<abs path to wallet home>
LEZ_ACCOUNT_ID=<this peer's base58 account id>
LEZ_HTLC_PROGRAM_ID=27720b5b0345135d8e684eb172c27f5fb237548cc891a3ec889d0ed340504070
LEZ_AMOUNT=1000
ETH_AMOUNT=0.0001
LEZ_TIMELOCK_MINUTES=20     # LEZ short — maker locks second
ETH_TIMELOCK_MINUTES=40     # ETH long — taker locks first; must be ≥ LEZ + margin
ETH_RECIPIENT_ADDRESS=<maker's ETH address>
LEZ_TAKER_ACCOUNT_ID=<taker's base58 account id>
POLL_INTERVAL_MS=2000
LEE_WALLET_HOME_DIR=<same as LEZ_WALLET_HOME>
```

Public-testnet cadence is ~30–60 s per LEZ block; the client allows 300 s for
lock/funding confirmation. Keep timelocks generous (≥20/≥40 minutes).

## First public swap — evidence (2026-07-21)

Preimage (identical on both chains): 
`78c150839eaad63312b3533d978fad7da860a5b9ddbeec6de2baf314e0654e1e` 
Hashlock: `ecca22673f2bba423a5689cfaf6d3f6d34dc076a57a8747b21f64714e06cee21`

| Leg | Chain | Tx |
|---|---|---|
| Taker locks 0.0001 ETH | Sepolia [`0xbf2364ac…`](https://sepolia.etherscan.io/tx/0xbf2364ac6b18bf6071242fd38a02a932089735d0f0ed88098a588798894c2df2) | block 11319359, swap id `0xebffdf016e5940338f53bafad13da936d44fd3a1c6957bc8f4c06594bc6247d4` |
| Maker locks escrow | LEZ testnet | `23570a637102933558c144a70a80a94898f3a7de1f1f90bc9b0f9f920541a147` (escrow PDA `Adt3mfTdqpD2CH1uiCSTZH4GLkHPEXsv14FnBr22Hnrw`) |
| Maker funds escrow (1000 LEZ) | LEZ testnet | `8dd92b8b50cf8d9039277b5cff724944e1ab6012a5ff60cc9bb604d96f519b3c` |
| Taker claims LEZ (reveals preimage) | LEZ testnet | `1992a354a993a43e3a8137f077fb987d0a50b3d7bd8f8e73e09b8a62a8f56136` |
| Maker claims ETH | Sepolia [`0x185ef038…`](https://sepolia.etherscan.io/tx/0x185ef038d80b27d173e4d5d564a7c05e06ccf6cca3b70cd42244236ffc89f9fb) | block 11319424 |

Peers: maker LEZ `AU4z2Ae7RFab1BFrWeCngeAp3Yq8P47jFC7x7zUK6pgv` /
ETH `0xF32eA5DD55a173700eA67777fa836aCe2E21B7b4`; taker LEZ
`BtVFJXs3uX6MNBuz6yzAQxsmLdW18MSVdcDY9XXjCgW6` /
ETH `0xb6aa7c4E43f698631cAcdE0be59C95Bec968f6E2`. Final settlement:
maker LEZ 1650→650, taker LEZ 0→1000; ETH moved 0.0001 taker→maker.

(The LEZ explorer at `explorer.testnet.lez.logos.co` was not reachable from
this environment; transactions were verified via the sequencer's
`getTransaction`/`getAccount` RPCs.)
