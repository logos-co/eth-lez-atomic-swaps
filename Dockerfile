# syntax=docker/dockerfile:1.7
#
# Maker liquidity-bot image: `swap-cli maker --loop` + its Node.js
# offer-publisher sidecar (offer-publisher/publish-offer.mjs), packaged
# together because swap-cli shells out to `node <script>` at runtime
# (src/cli/bot.rs:spawn_offer_publisher) — the two are one deployable unit,
# not two.
#
# Why a container at all (see deploy/README.md for the full rationale): the
# build needs Rust 1.93 with network-fetching build scripts (LEZ v0.2.0 deps)
# AND Node >=20 for offer-publisher/. Nix was rejected for this repo's CLI
# packaging (cargoHash churn, issue #32); a bare systemd unit was rejected
# because it leaves the toolchain as manual, drifting VPS state. This image
# is the reproducible middle ground.
#
# Three stages: Rust builder, Node builder (deps only — offer-publisher has
# no build step), and a slim node-based runtime (swap-cli itself spawns
# `node`, so the runtime needs a real Node install, not just the binary).

########################################
# Stage 1: swap-cli (Rust release build)
########################################
FROM rust:1.93-bookworm AS rust-builder
WORKDIR /build

# ca-certificates: LEZ v0.2.0 dependency build scripts fetch prebuilt
# circuit/rapidsnark artifacts over HTTPS at compile time (network build is
# the normal non-nix dev flow for this repo — see .github/workflows/ci.yml).
# pkg-config/libssl-dev: native TLS deps pulled in transitively.
# python3-dev: risc0-zkvm's dependency tree links `pyo3`/`pyo3-ffi` against
# libpython at compile time (unrelated to RISC0_SKIP_BUILD, which only
# skips building the guest ELF, not this native link requirement) — without
# it the final link step fails with "cannot find -lpython3.1x".
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        pkg-config \
        libssl-dev \
        python3-dev \
    && rm -rf /var/lib/apt/lists/*

# Workspace manifests first (members = [".", "swap-ffi", "lez-mcp"] in
# Cargo.toml) — cargo parses every member's Cargo.toml during workspace
# resolution even though we only build the swap-cli bin target, so all three
# member trees must be present.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY swap-ffi ./swap-ffi
COPY lez-mcp ./lez-mcp
COPY programs ./programs

# RISC0_SKIP_BUILD=1: without it, cargo reaches for the risc0 guest
# toolchain (rzup) while evaluating the workspace and breaks in a plain
# container — swap-cli's default build never needs the actual zkVM guest
# (that only exists behind the `demo` feature / lez_htlc_methods, an
# optional dep). Mirrors the same env var CI sets (.github/workflows/ci.yml).
ENV RISC0_SKIP_BUILD=1
RUN cargo build --release --locked --bin swap-cli

########################################
# Stage 2: offer-publisher/ dependencies (Node >=20)
########################################
FROM node:20-bookworm-slim AS node-builder
WORKDIR /build/offer-publisher

COPY offer-publisher/package.json offer-publisher/package-lock.json ./
RUN npm ci --omit=dev

COPY offer-publisher/fleet.mjs offer-publisher/publish-offer.mjs ./

########################################
# Stage 3: runtime
########################################
FROM node:20-bookworm-slim AS runtime

# procps: gives the compose healthcheck a `pgrep` to check swap-cli liveness
# with (bookworm-slim ships neither procps nor a /proc-walking alternative).
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
        procps \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 --shell /usr/sbin/nologin maker

COPY --from=rust-builder /build/target/release/swap-cli /usr/local/bin/swap-cli
COPY --from=node-builder /build/offer-publisher /app/offer-publisher

# Named volume mount point for durable maker state: the crash-recovery
# journal (.maker-state.json, see src/cli/maker.rs MAKER_STATE_FILE) and any
# status file the loop writes. MUST be a path under this volume, never the
# image's CWD-relative default — a container restart / redeploy would
# otherwise silently lose the in-flight-swap journal (see also the
# .gitignore fix in this same change: .maker-state.json was untracked but
# never actually ignored).
RUN mkdir -p /app/state && chown -R maker:maker /app
VOLUME ["/app/state"]

WORKDIR /app
USER maker

ENV MAKER_STATE_FILE=/app/state/.maker-state.json \
    OFFER_PUBLISHER_SCRIPT=/app/offer-publisher/publish-offer.mjs \
    RUST_LOG=info

# tini as PID 1: swap-cli's own SIGTERM handling (wait_for_shutdown_signal in
# src/cli/maker.rs) needs to actually receive the signal, and it in turn
# reaps the node sidecar child — but only if something reaps zombies /
# forwards signals correctly as PID 1 in the first place.
#
# --restrict-counterparty is deliberately NOT baked in here as a CLI flag —
# it is read from the RESTRICT_COUNTERPARTY env var (see deploy/maker.env)
# instead, so switching a deployment between restricted and public mode
# (once PR #64/#76 land and the Sepolia contracts are redeployed) is a single
# env-file change, not an image rebuild. See deploy/README.md.
ENTRYPOINT ["/usr/bin/tini", "--", "swap-cli"]
CMD ["maker", "--loop"]
