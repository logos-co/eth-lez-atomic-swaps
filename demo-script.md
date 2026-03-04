# Atomic Swaps — Demo Prep

## Comms Paragraph

Trustless cross-chain token exchange between LEZ and Ethereum, built entirely on the Logos stack. Two parties swap tokens across chains without a centralized exchange — hash time-locked contracts guarantee that either both sides complete or both get their money back. The LEZ side executes as a verifiable program in a zkVM, swap offers are discovered over Waku (peer-to-peer, no order book server), and the UI runs as a logos-app plugin. This is a working end-to-end application: local testnets, real smart contracts, real verifiable execution, with a desktop interface you can try yourself in five commands.

---

## Chair Presentation (~15 min)

**Before you start:** Have `make infra` running, two UI windows open side by side.

---

### Intro (~30 sec)

"Hey everyone, I'm Danish, core contributor at Logos. I've been working on a sample app that shows what you can build on the Logos stack — specifically cross-chain interop. I'm going to walk through what it does, show you a live demo, and then talk about where it could go from here."

---

### 1. What Does It Do + Properties Thanks to Logos Stack (~3 min)

"So this is a cross-chain atomic swap between LEZ and Ethereum. Maker has LEZ, taker has ETH — they want to trade. No exchange, no bridge, no middleman."

```
Maker                                    Taker
  |  1. Generate secret, lock LEZ         |
  |──────── share hash ─────────────────>|
  |                           2. Verify LEZ lock, lock ETH
  |  3. Claim ETH (reveals secret)        |
  |                           4. Claim LEZ (using revealed secret)
```

"Hash time-locked contracts on both chains guarantee: either both sides complete or both get refunded. That's the whole trust model."

"Quick note on why HTLCs — we considered a few approaches. A centralized exchange is the obvious one but you're trusting a middleman, which defeats the purpose. AMMs need liquidity pools and on-chain infrastructure on both sides, which is way too heavy for a PoC. Adaptor signatures (Schnorr-based) are the more elegant cryptographic approach and better for privacy, but they're more complex to implement and require specific signature scheme support. HTLCs are the simplest trust-minimized primitive — they work with any chain that can verify a hash, which made them the right starting point. Adaptor signatures are the natural next step."

"What makes this more than a swap demo is what it's built on — every layer is a Logos component:"

- **LEZ / LSSA** — the LEZ HTLC is a Risc0 zkVM guest program. Every state transition is verifiably executed. The escrow is a program-derived address that only the HTLC program can modify — enforced by the LSSA execution model.
- **Waku** — offer discovery is peer-to-peer. Maker publishes an offer to a Waku content topic, taker discovers it by subscribing. No order book server.
- **logos-app** — the UI is a logos-app IComponent plugin. Same codebase runs standalone or inside logos-app. It's a non-trivial plugin — FFI bridge to Rust, async swap flows, real-time progress callbacks — and it works.
- **SHA-256 hashlock** (not keccak) — Risc0 has native SHA-256, Solidity has a cheap sha256 precompile. Both chains verify the same hash natively.

**Normies can drop here.** Takeaway: you can build real cross-chain apps on the Logos stack today.

---

### 2. UX Demo (~5 min)

"Let me show you it running."

**Maker window:**
1. Show the Config tab briefly — chain endpoints, amounts, timelocks all pre-filled from `.env`
2. Go to Maker tab, click **Publish Offer** — this broadcasts via Waku
3. Offer card appears with the hashlock. Click **Start Swap**
4. Progress stepper ticks through: Generate Preimage -> Lock LEZ -> Wait for ETH Lock

**Taker window:**
1. Go to Taker tab, click **Discover Offers** — the maker's offer appears (came over nwaku)
2. Click the offer — hashlock auto-fills
3. Click **Start Taker**
4. Progress stepper: Verify LEZ Escrow -> Lock ETH -> Wait for Preimage

**Watch both complete:**
- Maker detects ETH lock, claims ETH (reveals preimage on Ethereum)
- Taker detects preimage from the Claimed event, claims LEZ
- Both windows show **"Swap Completed"** with tx hashes

"The whole thing takes ~10-15 seconds on local testnets."

`make demo` does the same thing headlessly in one terminal if you just want to verify it works.

**Architecture bits to mention while the demo runs (during the wait steps):**

- The orchestration library is interface-agnostic Rust — the maker and taker flows are ~120 lines each. The UI, CLI, and integration tests all call the same library.
- Chain monitoring is polling-based: ETH watcher polls for contract events via alloy, LEZ watcher polls the escrow PDA via the sequencer API.
- The FFI bridge (`swap-ffi`) exposes a C API to Qt6. Progress updates flow back as JSON callbacks.
- The off-chain LEZ timelock is a pragmatic PoC choice — LSSA doesn't expose block time yet, so the orchestration library checks wall-clock time before submitting refunds.
- The two-step LEZ lock (Lock then Transfer) is because LSSA's ownership model requires claiming the PDA before funding it — otherwise the transfer program takes ownership.

---

### 3. Next Steps / Limitations (~3 min)

"Being honest — it's a PoC, there are clear limitations:"

- Hardcoded swap params — no price negotiation or order matching
- Single swap per instance
- Off-chain LEZ timelock — LSSA doesn't expose timestamps yet
- Cross-chain linkability — the shared hashlock correlates both sides on-chain, even if LEZ accounts are private
- No crash recovery — process dies mid-swap, you manually refund via CLI

"If this continues, the interesting next steps:"

- **Adaptor signatures** (Schnorr-based) — removes the hashlock linkability problem entirely, big privacy win
- **On-chain LEZ timelocks** when LSSA exposes block time
- **Waku order book** — multiple makers publishing competing offers, takers pick the best rate
- Persistent swap state + auto-recovery
- ERC-20 support (currently native ETH/LEZ only)

---

### 4. Steps to Hack on It (~2 min)

"Five commands to run this yourself:"

```bash
git clone --recurse-submodules https://github.com/logos-co/eth-lez-atomic-swaps.git
cd eth-lez-atomic-swaps
make infra            # spins up Anvil (local Ethereum), LEZ sequencer, nwaku,
                      # deploys HTLC contracts on both chains, writes .env files
# new terminal:
make configure        # builds the Rust FFI bridge + cmake configure for the Qt6 app (first time only)
make run-maker        # launches the maker UI, loads config from .env
# another terminal:
make run-taker        # launches the taker UI, loads config from .env.taker
```

"You need Rust 1.85+, Foundry, CMake, Qt6, Docker. README has install instructions for macOS and Linux."

"A few other useful commands:"

- `make demo` — runs the full swap headlessly in one terminal, no UI needed, good sanity check
- `make plugin-build` / `make plugin-run` — builds and runs as a logos-app plugin instead of standalone
- `make nwaku-stop` — tears down the Docker containers when you're done

"If you want to dig into the code:"

- `src/swap/maker.rs` + `taker.rs` — the core swap flows, readable top-to-bottom
- `contracts/src/EthHTLC.sol` — the Ethereum HTLC, `forge test` runs it
- `programs/lez-htlc/methods/guest/src/main.rs` — the LEZ HTLC with inline tests
- `ui/qml/MakerView.qml` + `TakerView.qml` — the UI views

---

## 5-10 min Video (separate recording for comms)

Different audience — external, may not know Logos. Structure: what / why / how.

**What (1 min):** Show two UI windows side by side. Run the swap. Let it complete. "Cross-chain atomic swap between LEZ and Ethereum. Two parties trade tokens across chains, trustlessly."

**Why (2 min):** Most cross-chain bridges require trust — multisigs, oracles, committees. Atomic swaps are the simplest trustless primitive. This PoC shows the Logos stack is ready for real cross-chain apps.

**How (3-5 min):** Walk through the swap flow in the UI. Point out the progress steppers — each step maps to an on-chain action. Show the architecture diagram. Mention: zkVM execution on LEZ, Waku for messaging, logos-app plugin. Show the five clone-and-run commands.

**Close:** "Source is open on GitHub. Clone it, run `make infra`, try it yourself."

---

## Pre-Demo Checklist

- [ ] `make infra` running (Docker up, no port conflicts)
- [ ] Both UI windows open and responsive
- [ ] Dry-run the full swap at least once
- [ ] `docker compose ps` shows nwaku healthy
- [ ] Font size readable for screen share / recording
- [ ] README and architecture diagram ready to show
