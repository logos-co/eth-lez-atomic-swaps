# Nix binary cache for CI

CI builds this repo's modules (`swap`, `swap_ui`) and the Basecamp runtime with
Nix. Their closures include large, slow-changing dependencies — the risc0
toolchain, Qt, and the liblogos chains — that used to be **rebuilt cold on every
job** (20–60 min) even when a pull request only touched app code.

A [Nix binary cache][attic] fixes that: unchanged store paths are pulled from the
cache instead of rebuilt. This document describes the cache, how CI is wired to
it, and how to migrate it to permanent infrastructure.

> **This cache is a STOPGAP.** It runs on the project owner's VPS. The long-term
> home is Logos org infrastructure (an org-wide attic, a Cachix org plan, or
> infra-hosted equivalent). CI is wired so that migration is a **two-value
> configuration change** — see [Migration](#migrating-to-permanent-infra).

## What CI uses

The cache URL and its public key are **repository variables**, never hardcoded in
a workflow:

| Repo variable / secret | Purpose | Current value |
| --- | --- | --- |
| `vars.NIX_CACHE_URL` | Substituter URL (pull) | `https://cache.substratestudios.xyz/swaps` |
| `vars.NIX_CACHE_PUBLIC_KEY` | NAR signature trust (pull) | `swaps:Q+9BFcsoB7lgdAoiyT/+ojIrTButT0H4BnjCSesltiQ=` |
| `secrets.NIX_CACHE_PUSH_TOKEN` | Push authorization (write) | *(secret)* |

- **Reads are public** — pulling needs no token. Any job (including PRs from
  forks) gets the substituter via `extra-substituters` /
  `extra-trusted-public-keys` in the Nix install step.
- **Writes require the token.** Only `push` events on `master` push their build
  outputs (via [`ryanccn/attic-action`][attic-action]); pull requests never
  push. Fork PRs cannot read the secret, by design.

Wiring per workflow:

| Workflow | Pull (substituter) | Push |
| --- | --- | --- |
| `build-modules.yml` (the producer) | **no** — kept cold on purpose to preserve its release-parity cold-build measurement | yes, on `push` to `master` |
| `canary.yml` (`modules` leg) | yes | yes, on `push` |
| `ci.yml` | yes (proposed diff — applied out of band) | — |

`build-modules.yml` deliberately does **not** pull: its whole reason to exist is
measuring the cold cost of the release build, which itself has no cache. It is
the ideal *producer* (it builds all three variants cold on every master push),
so it pushes but never substitutes.

## The server

`atticd` ([attic][attic]) in Docker on the owner's VPS, reverse-proxied by the
existing Caddy at `https://cache.substratestudios.xyz/`.

- **Storage/dedup:** local storage with content-defined chunking, so the
  slow-changing deps are stored once across builds and variants.
- **Garbage collection:** unreferenced NARs older than **14 days** are collected
  every 12 h. Soft disk budget ~25 GB (the VPS also runs the maker; the cache is
  sized to leave well over 20% headroom). Tighten the retention window if disk
  pressure appears.
- **Trust model:** the JWT signing secret (root of trust for tokens) lives
  **only on the VPS** (root-readable env file, never in git or GitHub). The push
  token is minted from it, scoped to push/pull on the `swaps` cache only, and
  stored as `secrets.NIX_CACHE_PUSH_TOKEN`. The NAR *signing* keypair is managed
  by attic per-cache; its public half is `vars.NIX_CACHE_PUBLIC_KEY`.

## Migrating to permanent infra

To move off the VPS, the infra provider supplies three things:

1. a substituter URL,
2. a NAR-signing public key, and
3. a write token (or push credential).

Then, in this repo:

- set `vars.NIX_CACHE_URL` → new substituter URL,
- set `vars.NIX_CACHE_PUBLIC_KEY` → new public key,
- set `secrets.NIX_CACHE_PUSH_TOKEN` → new write token.

No workflow edits are needed for the **pull** side (every PR's win) — it is pure
configuration. The **push** side uses `ryanccn/attic-action`; if the new home is
another attic it works unchanged, and if it is Cachix the push step is swapped
for `cachix/cachix-action` (the pull side still stays pure config).

To **decommission** the VPS cache: remove the `cache.substratestudios.xyz` vhost
from the Caddyfile, `docker compose down` the `attic` service under
`/opt/services/attic`, and delete its data directory.

[attic]: https://github.com/zhaofengli/attic
[attic-action]: https://github.com/ryanccn/attic-action
