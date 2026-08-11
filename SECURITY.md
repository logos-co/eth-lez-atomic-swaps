# Security

This document tracks the supply-chain and infrastructure posture of
`eth-lez-atomic-swaps`. It has two parts:

1. **Hardening already in the repo** — controls that ship as code and CI.
2. **Owner-action items** — decisions and infrastructure changes that are **not
   code we can merge here**. They require a repository/organization owner to act
   *before this project is opened to public scale*. They are listed in priority
   order.

## Reporting a vulnerability

Report suspected vulnerabilities privately to the repository maintainers (do not
open a public issue for an unpatched flaw). Include the affected component, a
reproduction, and the impact.

## Hardening in this repo (merged)

- **Canary channel is fenced from the official catalogue.** Canary
  (`canary-channel.yml`) publishes branch builds as `.lgxc` assets on the
  `canary` prerelease, an extension the official `rebuild-index` enumerator
  (`endswith(".lgx")`) does not collect — so a canary build can no longer leak
  into the official `index.json`. A defence-in-depth guard,
  `canary/leg-index-fence.sh`, fails CI if the live official index ever contains
  a `0.99.x` sentinel version or a `canary`-release URL. See `canary/README.md`.
- **Release provenance gate.** `release-swap.yml` / `release-swap-ui.yml` refuse
  to cut an official release unless the dispatched commit is reachable from
  `origin/master` (`git merge-base --is-ancestor`). This blocks dispatching an
  official release from a side branch carrying an unreviewed commit. It proves
  the commit is in master's history — **not** that the artifact is authentic;
  see owner-action (a).

## Owner-action items (requires owner decision — NOT merged here)

These are prioritized. Each is an owner/infra decision, not a code change this PR
can make.

### (a) P0 — Modules ship UNSIGNED (`trustedSigners: []`)

Both repository descriptors (`logos-repo.json`, `logos-repo-canary.json`) carry
`"trustedSigners": []`, and the release workflows use `signing_mode: none`
(unsigned `.lgx`). Basecamp therefore installs whatever bytes the advertised URL
serves, with no cryptographic proof of publisher. The release-provenance gate
above narrows *who can cut a release from what commit*, but it cannot attest the
artifact a client downloads.

**Before public scale:** provision a module-signing key, switch the release
workflows to `signing_mode: inline` (or `external`), and populate
`trustedSigners` in both descriptors with the corresponding public key(s). Until
then the catalogue's integrity rests entirely on GitHub account + Actions
security.

### (b) P1 — VPS co-locates funded keys, cache-signing secret, catalogue write path, and a self-hosted runner under one user

The deployment VPS currently holds, under a single user account:

- funded **maker private keys**,
- the **attic/nix cache signing secret**,
- the **public catalogue write path** (the token/credential able to publish
  releases and the rolling `index`), and
- a **self-hosted GitHub Actions runner**.

Any one compromise (a runner job escape, a leaked token, a maker-key theft)
reaches all four. A self-hosted runner in particular executes arbitrary
workflow code and is the softest entry point.

**Before public scale:** decouple these onto least-privilege boundaries — funds,
cache-signing, and catalogue-publish should live on separate principals/hosts,
and the self-hosted runner (if kept) should run as an unprivileged user with no
access to funded keys or signing secrets. Prefer GitHub-hosted runners for
release/publish jobs where feasible.

### (c) FLAG — discord-cockpit daemon binds `0.0.0.0:8899`, bypassing Caddy basic_auth

**Not ours to fix — flagged for the host owner.** The `discord-cockpit` daemon on
that VPS binds `0.0.0.0:8899`. Caddy fronts the service with `basic_auth`, but
because the daemon listens on all interfaces, the port is reachable directly,
bypassing Caddy's authentication entirely.

**Owner action:** verify the host firewall blocks `8899` from the public
internet, or rebind the daemon to `127.0.0.1:8899` so only Caddy can reach it.
Confirm with an external port scan.
