# Local Basecamp dev harness (`make basecamp-dev`)

The fastest way to see a **working-tree** change to `swap` or `swap_ui` running
in the **real Basecamp** on your Mac — no CI, no catalogue, no module-manager
service, no release. Build → stage → launch in **~3-6 minutes warm**.

```sh
make basecamp-dev              # build working tree + stage + launch Basecamp
# ...edit swap-module/ or swap-ui/...
make basecamp-dev              # again — the corner badge shows the new -dev.<sha>
```

That is the whole loop. Everything below is context.

---

## Why this exists — the feedback cycle, and the failure class it kills

| Path | Turnaround | What it proves |
| --- | --- | --- |
| Official catalogue install | **~2.5 h** | Full distribution: build → release → catalogue publish → module-manager install |
| Canary channel ([PR #118](https://github.com/logos-co/eth-lez-atomic-swaps/pull/118)) | **~25 min** | A merged branch build, installed into Basecamp minutes after CI |
| **`make basecamp-dev` (this)** | **~3-6 min** | Your **uncommitted** module code, loaded into real Basecamp |

Three separate times, the owner's Basecamp kept **LOADING a stale module** after
a catalogue update — the GUI gave no way to tell which build was actually live,
so a fixed bug looked unfixed (or vice-versa). This harness kills that failure
class two ways:

1. It bypasses the catalogue/module-manager entirely and installs your
   working-tree build directly into an **isolated dev profile**, so there is no
   stale-cache codepath to load from.
2. It **stamps a distinct `-dev.<sha>` version** into every staged manifest, so
   Basecamp's corner version badge *proves* which build is live. If the badge
   doesn't read the `-dev.<sha>` the harness printed, you are looking at the
   wrong window — not a mystery anymore.

---

## Usage

```sh
make basecamp-dev                       # default: build, stage, launch
make basecamp-dev ARGS=--skip-build     # reuse last artifacts (skip nix build)
make basecamp-dev ARGS=--no-launch      # stage + verify + print launch command
make basecamp-dev ARGS=--pinned-basecamp   # force scaffold's pinned bundle
make basecamp-dev ARGS=--installed-app     # force the installed /Applications app
```

Or call the script directly: `bash scripts/basecamp-dev.sh [flags]`.

### Environment overrides

| Variable | Default | Purpose |
| --- | --- | --- |
| `DEV_ROOT` | `~/.eth-lez-dev` | Dev root: isolated user-dir + artifact cache |
| `BASECAMP_APP` | first `/Applications/*Basecamp*.app` | Basecamp bundle to launch |
| `DELIVERY_MODULE_LGX` | (built from the scaffold pin) | Pre-built `delivery_module` `.lgx` to stage instead of building |

### What a run does

1. Builds scaffold's **pinned `lgpm`** (cached across runs).
2. Builds `swap` and `swap_ui` from the **working tree**
   (`nix build ./swap-module#lgx-portable`, `./swap-ui#lgx-portable`).
3. Resolves `delivery_module` (override → cache → build the scaffold pin).
4. Installs all three into `~/.eth-lez-dev/profile/{modules,plugins}` via the
   pinned `lgpm` — the same install path as `make basecamp-ui-smoke` and
   `lgs basecamp install`, so `view` + `hashes` round-trip correctly.
5. **Stamps** `swap` and `swap_ui` manifests to `<version>-dev.<sha>` (a
   `.dirty` marker is appended when the working tree has uncommitted changes).
6. **Self-verifies** (below), then launches Basecamp against the dev profile.

### Self-verification (before it declares "ready")

The script fails loudly rather than launch a broken stage. It checks:

- all three manifests exist (`delivery_module`, `swap`, `swap_ui`);
- `swap` and `swap_ui` carry the `-dev.<sha>` stamp;
- `swap` kept its `delivery_module` dependency;
- `swap_ui` is a `ui_qml` plugin with a non-empty `view` (Basecamp 0.2.x
  hard-filters `ui_qml` plugins with an empty `view`);
- every staged `.dylib` is **arm64** (a stale x86_64 artifact is exactly the
  silent-load failure this harness exists to catch).

GUI verification stays with you — deliberately. The script never drives or
inspects the launched GUI (that automation lives in `make basecamp-ui-smoke`);
the stamp resolves the "which build is live?" question faster than any
automation could. It prints the one line that matters:

```
>>> corner version badge should read: 0.4.2-dev.<sha> <<<
```

---

## When to use which

- **`make basecamp-dev`** — you are iterating on `swap` / `swap_ui` C++ or QML
  and want to *see it in Basecamp now*. Tests **module code**, not distribution.
- **Canary channel** — you want a teammate (or yourself, cleanly) to install a
  **merged branch** build the way a real user would, minutes after CI.
- **Official catalogue** — release validation: the full build → release →
  catalogue → module-manager install path a shipped user actually takes.
- **`make basecamp-ui-smoke`** — headless, hermetic CI gate (Linux): package
  discovery, dependency wiring, `logos_host` / `ui-host` startup, QML load, tab
  navigation. Run it before you push; run `basecamp-dev` while you code.

---

## Relationship to scaffold (`lgs`) — reused vs. filled

This harness is a **thin composition** over scaffold, not a re-port of it.

**Reused from scaffold:**

- `scaffold.toml` is the single source of truth for the `delivery_module`,
  `lgpm`, and `basecamp` pins — the script reads them, hardcodes nothing.
- Scaffold's pinned **`lgpm cli-portable`** does the install.
- Scaffold's pinned **basecamp bundle** is the preferred launch target when it
  is already realizable from the Nix store.
- The Makefile's layout-drift guard pattern is reused before launch.

**Gaps this harness fills** (candidate scaffold feature requests — see
`docs/scaffold-upstream-tracker.md`):

1. **Working-tree builds.** `lgs basecamp build`/`install` build the *captured*
   (committed) source set — the `git+file:.?dir=…` refs in `scaffold.toml`. A
   dev loop needs the **live working tree**, so the harness builds the path
   flake refs `./swap-module#lgx-portable` / `./swap-ui#lgx-portable` directly.
   → *FR: `lgs basecamp install --working-tree` (build the path ref, not the
   captured git ref).*
2. **Dev-version stamp.** Nothing in `lgs` stamps an installed module so the
   corner badge proves which build is live.
   → *FR: `lgs basecamp install --stamp-version dev.<sha>`.*
3. **No-scrub launch.** `lgs basecamp launch` scrubs the profile and **replays**
   the captured module set on every invocation, which would wipe the stamp. The
   harness launches Basecamp directly against the already-staged, stamped
   profile.
   → *FR: `lgs basecamp launch --no-scrub` (launch the profile as-is).*

---

## Limitations — what this does NOT exercise

- **Not the distribution path.** The catalogue, the module manager, signing,
  and release packaging are all bypassed. This tests **module code**, not how a
  user *gets* the module. Use the canary channel / catalogue for that.
- **Not the pinned basecamp by default.** For speed the harness launches the
  installed `/Applications` app when it matches, or the scaffold pin when it is
  already in the Nix store. Force the exact reproducible pin with
  `--pinned-basecamp` (may trigger a long first-time macOS build).
- **No localnet, Anvil, wallet funding, or LEZ sequencer.** This proves the
  modules *load and render*, not that a funded two-peer swap settles. Use
  `make basecamp-ui-smoke`, `make test`, or the two-profile
  `make basecamp-launch-maker` / `-taker` flow for behaviour.
- **macOS / arm64 only.** On Linux use `make basecamp-ui-smoke`.
- **GUI check is manual.** The script verifies files; you confirm the badge and
  the flow in the window it launches.
