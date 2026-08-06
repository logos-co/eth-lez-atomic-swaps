{
  description = "LEZ <> ETH atomic swap module (universal C++ wrapping swap-ffi)";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    nix-bundle-lgx.url = "github:logos-co/nix-bundle-lgx";
    delivery_module = {
      url = "github:logos-co/logos-delivery-module/v0.1.1";
      # crates.io answers HTTP 403 to the User-Agent nixpkgs' cargo vendor
      # fetcher sends (`python-requests/*`; `curl/*` is blocked too — both
      # measured, a custom UA gets 200). Upstream fixed it in
      # NixOS/nixpkgs#512735 (2026-04-26) and later moved the download to
      # static.crates.io; neither change was backported to release-25.05 or
      # release-25.11.
      #
      # logos-delivery pins its OWN nixpkgs (`23d72dab`, release-25.11,
      # 2026-02-07) outside this flake's follows graph, and that is the
      # nixpkgs that vendors zerokit/rln — so every leg of every build died
      # in `zerokit-0.9.0-vendor-staging` no matter what the rest of the
      # graph did. This nested follows is the only lever a consumer has, and
      # unlike `--override-input` it survives the bare `nix build
      # .#lgx-portable` the shared release workflow runs.
      #
      # nixos-26.05 is the oldest *released* channel carrying the fix (a
      # channel rev, not release-26.05 HEAD, so cache.nixos.org has the whole
      # closure for cold CI runners). Verified locally that it keeps
      # zerokit's committed `cargoHash = "sha256-WXxQ8mAPD/..."` valid: the
      # 26.05 fetcher and a 23d72dab+User-Agent patch produce a
      # byte-identical vendor-staging tree. Only the request changes.
      #
      # logos-delivery's own `zerokit.inputs.nixpkgs.follows = "nixpkgs"`
      # carries this down to zerokit. Drop the whole thing once
      # logos-messaging/logos-delivery advances its nixpkgs past 2026-04-26.
      inputs.logos-delivery.inputs.nixpkgs.follows = "nixpkgs-crates-io";
      # issue #70: swap zerokit itself for `zerokit-nocheck` below, which is
      # the same zerokit at the same rev with the `rln` package's checkPhase
      # disabled. logos-delivery-module's own flake ties its top-level
      # `zerokit` input to `logos-delivery/zerokit` (`zerokit.follows =
      # "logos-delivery/zerokit"`), so overriding it here at the
      # logos-delivery level also covers the librln.dylib copy
      # logos-delivery-module bundles directly — one override reaches both
      # consumers. See nix/zerokit-nocheck/flake.nix for why.
      inputs.logos-delivery.inputs.zerokit.follows = "zerokit-nocheck";
    };
    nixpkgs.follows = "logos-module-builder/nixpkgs";
    # Used ONLY as the `follows` target above. Nothing in this flake's own
    # outputs is built from it, so it does not move the module's toolchain.
    nixpkgs-crates-io.url = "github:NixOS/nixpkgs/21ea275a7c46aef9d4d6ddc962e6d562e9d94183";
    # issue #70: zerokit's `rln` package with its checkPhase (`cargo test -p
    # rln`) disabled — see nix/zerokit-nocheck/flake.nix. Kept on the same
    # crates.io-403-fixed nixpkgs/rust-overlay chain as the rest of this
    # input block so it doesn't regress issue #32.
    zerokit-nocheck = {
      url = "path:./nix/zerokit-nocheck";
      inputs.zerokit.inputs.nixpkgs.follows = "nixpkgs-crates-io";
      inputs.zerokit.inputs.rust-overlay.follows = "rust-overlay";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    swap-source = {
      url = "path:..";
      flake = false;
    };
  };

  outputs = inputs@{ logos-module-builder, nixpkgs, rust-overlay, swap-source, ... }:
    let
      lib = nixpkgs.lib;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];

      swapFfiPackages = lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          dylibName = if pkgs.stdenv.hostPlatform.isDarwin then "libswap_ffi.dylib" else "libswap_ffi.so";
          rustToolchain = pkgs.rust-bin.stable."1.93.0".default;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          # The same crates.io 403 the `delivery_module` input comment
          # describes, on the *other* nixpkgs chain: swap-ffi is vendored by
          # the nixpkgs this flake follows (`e9f00bd8`, 2025-09), which also
          # predates NixOS/nixpkgs#512735, so
          # `swap-ffi-0.1.0-vendor-staging` 403s on its first crate.
          #
          # Repinning is not an option here the way it is for logos-delivery:
          # this nixpkgs is the module's entire Qt/C++ toolchain. So instead
          # this is nixpkgs' `fetch-cargo-vendor.nix` re-expressed here, with
          # its two helper scripts rebuilt from the pinned tree and one header
          # line added to the fetching one. Only swap-ffi uses it; nixpkgs'
          # own `rustPlatform` is untouched, so no other derivation in the
          # graph moves and nothing loses its binary-cache hit. A request
          # header cannot change the vendored tree, so `cargoDeps.hash` below
          # is the same value a plain `cargoHash` takes.
          #
          # Deliberately re-expressed rather than generated with
          # `builtins.toFile` from the original: a generated file has to embed
          # `${nixpkgs}` store paths as text, and registering those as
          # references made evaluation die with `path '…-source' is not valid`
          # on CI's Determinate Nix (run 30618639995, all six legs) while
          # evaluating clean on stock Nix. Reading the scripts with
          # `builtins.readFile` keeps every store reference out of it.
          #
          # The substitutions are asserted, so a nixpkgs bump that reshapes the
          # fetcher fails evaluation loudly instead of silently going back to
          # a 403. Delete all of this once logos-module-builder's nixpkgs
          # carries the upstream fix (logos-co/logos-module-builder#173).
          #
          # SECOND substitution: crates.io 429. Once the 403 was fixed and the
          # build-modules matrix started fanning six legs out at once, each
          # vendoring ~850 crates from one runner IP range, crates.io began
          # throttling: `Failed to fetch file from
          # https://crates.io/api/v1/crates/rustc-hash/2.1.2/download. Status
          # code: 429` (run 30657933051 — 3 of 6 legs; whichever leg won the
          # race finished). Upstream ALREADY mounts a urllib3 `Retry`, but its
          # `status_forcelist` is 5xx-only, so the one status that explicitly
          # means "back off and come back" is the one status it does not retry.
          # Widen it, and make the backoff suit a shared throttle:
          #   * 429 added; 5xx kept. 403 deliberately NOT retried — that is a
          #     policy answer (the User-Agent block above), and retrying it
          #     would just burn the 90-minute timeout to reach the same error.
          #   * jitter, so six legs that all trip the limit in the same second
          #     do not then retry in lockstep and re-trip it together.
          #   * `respect_retry_after_header` (urllib3's default, pinned
          #     explicitly) so a `Retry-After` from crates.io wins over our
          #     own backoff curve.
          #   * bounded: 12 attempts, backoff capped at 60s => ~7 min of wait
          #     per URL worst case. A genuinely down registry still fails, it
          #     just fails after minutes instead of hanging out to 90.
          # `backoff_jitter`/`backoff_max` are urllib3 >= 2.0 kwargs; the
          # pinned tree has urllib3 2.5.0.
          #
          # THIRD substitution, and the one that actually fixes both: fetch the
          # tarballs from `static.crates.io` instead of the `crates.io/api/v1`
          # endpoint. Retry alone was measured to be insufficient — run
          # 30668452716 rode the full ~7.5 min backoff and still came out with
          # `requests.exceptions.RetryError: ... Max retries exceeded with url:
          # /api/v1/crates/rlp/0.5.2/download (Caused by ResponseError('too
          # many 429 error responses'))`, because the throttle outlasts any
          # bounded budget when six legs pull ~850 crates each through a pool
          # of 5. `static.crates.io` is the CDN cargo itself downloads from
          # (the registry's `dl` key); it applies neither the User-Agent policy
          # that produced the 403 nor the rate limit that produces the 429, so
          # it removes the cause instead of waiting the symptom out. This is
          # also where upstream nixpkgs moved after #512735.
          #
          # Byte-identical to the API endpoint, verified: `rlp` 0.5.2 is 14181
          # bytes from both, sha256 bb919243…e1ec, matching this repo's
          # Cargo.lock exactly (likewise rustc-hash 2.1.2 -> 94300abf…dbe, the
          # crate that 429'd). The script checksums every tarball against
          # Cargo.lock anyway, so a wrong URL fails loudly rather than
          # silently vendoring something else.
          #
          # The retry stays as a belt: the CDN can still 5xx, and a 429 from
          # any future URL change should back off rather than fail cold.
          #
          # These are the fixes the RELEASE workflow needs too — it fans out
          # the same way over the same registry, so they belong in the build,
          # not in the CI matrix. Kept as scoped `replaceStrings` on our
          # private copy of the script rather than an `applyPatches` of the
          # shared `fetch-cargo-vendor-util`: patching the real one moves the
          # `outPath` of every Rust package in nixpkgs (measured:
          # `qt6.qtdeclarative`, `python3Packages.cryptography` and
          # `cargo-auditable` all shift), which would cost this build its
          # binary-cache hits on exactly the cold CI runners it is meant to
          # help. The scoped form leaves every other outPath untouched.
          #
          # No substitution can change the bytes that get downloaded, so
          # `cargoDeps.hash` below is unaffected by all three.
          fetchCargoVendorUA =
            let
              rustBuildSupport = "${nixpkgs}/pkgs/build-support/rust";
              subst = what: from: to: text:
                let out = builtins.replaceStrings [ from ] [ to ] text; in
                if out == text
                then throw "swap-ffi: crates.io ${what} workaround is stale — ${builtins.toJSON from} not found in nixpkgs' cargo vendor fetcher"
                else out;
              replaceWorkspaceValues = pkgs.writers.writePython3Bin "replace-workspace-values" {
                libraries = with pkgs.python3Packages; [ tomli tomli-w ];
                flakeIgnore = [ "E501" "W503" ];
              } (builtins.readFile "${rustBuildSupport}/replace-workspace-values.py");
              fetchCargoVendorUtil = pkgs.writers.writePython3Bin "fetch-cargo-vendor-util" {
                libraries = with pkgs.python3Packages; [ requests ];
                flakeIgnore = [ "E501" ];
              } (subst "User-Agent"
                   "    session = requests.Session()\n"
                   "    session = requests.Session()\n    session.headers[\"User-Agent\"] = \"nixpkgs-fetchCargoVendor/1 (https://github.com/NixOS/nixpkgs)\"\n"
                (subst "429 retry"
                   "        total=5,\n        backoff_factor=0.5,\n        status_forcelist=[500, 502, 503, 504]\n"
                   "        total=12,\n        backoff_factor=1.5,\n        backoff_jitter=1.0,\n        backoff_max=60,\n        respect_retry_after_header=True,\n        status_forcelist=[429, 500, 502, 503, 504]\n"
                (subst "static.crates.io download"
                   "    return f\"https://crates.io/api/v1/crates/{pkg[\"name\"]}/{pkg[\"version\"]}/download\"\n"
                   "    return f\"https://static.crates.io/crates/{pkg[\"name\"]}/{pkg[\"name\"]}-{pkg[\"version\"]}.crate\"\n"
                   (builtins.readFile "${rustBuildSupport}/fetch-cargo-vendor-util.py"))));
            in
            { name, hash, ... }@args:
            let
              vendorStaging = pkgs.stdenvNoCC.mkDerivation ({
                name = "${name}-vendor-staging";

                impureEnvVars = lib.fetchers.proxyImpureEnvVars;

                nativeBuildInputs = [
                  fetchCargoVendorUtil
                  pkgs.cacert
                  # break loop of nix-prefetch-git -> git-lfs -> asciidoctor ->
                  # ruby (yjit) -> fetchCargoVendor -> nix-prefetch-git
                  (pkgs.nix-prefetch-git.override { git-lfs = null; })
                ];

                buildPhase = ''
                  runHook preBuild
                  fetch-cargo-vendor-util create-vendor-staging ./Cargo.lock "$out"
                  runHook postBuild
                '';

                strictDeps = true;
                dontConfigure = true;
                dontInstall = true;
                dontFixup = true;

                outputHash = hash;
                outputHashMode = "recursive";
              } // builtins.removeAttrs args [ "name" "hash" ]);
            in
            pkgs.runCommand "${name}-vendor"
              {
                inherit vendorStaging;
                nativeBuildInputs = [ fetchCargoVendorUtil rustToolchain replaceWorkspaceValues ];
              }
              ''
                fetch-cargo-vendor-util create-vendor "$vendorStaging" "$out"
              '';
          # logos-blockchain-circuits prebuilt artifact. Version must match the
          # logos-blockchain-circuits-* crates in the workspace lock (v0.5.3 at
          # the LEZ v0.2.2 pin); their build scripts resolve it via
          # LBC_ROOT_DIR (the pre-0.5 LOGOS_BLOCKCHAIN_CIRCUITS env is gone)
          # and would otherwise try to download it inside the sandbox.
          #
          # Platform tag and hash live in one attrset so the two can't drift.
          # x86_64-darwin is absent on purpose: upstream publishes no
          # macos-x86_64 bundle for v0.5.3, so Intel macOS is unsupportable
          # here (README "Prerequisites" says the same).
          circuitsAsset = {
            aarch64-darwin = {
              platform = "macos-aarch64";
              sha256 = "0w3i0phgzjswsk1q2k6cr3001jjc55a82z79zw9w5p3x9hwaqljq";
            };
            x86_64-linux = {
              platform = "linux-x86_64";
              sha256 = "1mwy3g9dyjvlwykzs62gzf79rrnm20sy7c587nv26c1y9bm71wfv";
            };
            aarch64-linux = {
              platform = "linux-aarch64";
              sha256 = "14r4vghipk66k8g22kymy2gpfa1ghwwa74v57a230yk0pm9zvgp7";
            };
          }.${system} or (throw "logos-blockchain-circuits is not published for ${system}");
          circuits = pkgs.fetchzip {
            url = "https://github.com/logos-blockchain/logos-blockchain-circuits/releases/download/v0.5.3/logos-blockchain-circuits-v0.5.3-${circuitsAsset.platform}.tar.gz";
            inherit (circuitsAsset) sha256;
          };
          # Prebuilt rapidsnark static libs for rust-rapidsnark's build.rs
          # (pulled in at the v0.2.2 pin via logos-blockchain-circuits-prover,
          # which forces the static-rapidsnark feature). Outside nix the build
          # script downloads these itself (see download_rapidsnark.sh in
          # logos-blockchain-rust-rapidsnark); the sandbox has no network, so
          # we prefetch the same per-target release asset and point
          # RAPIDSNARK_LIB_DIR at its lib/ dir.
          #
          # The Linux legs point at logos-blockchain's `-pic` rebuilds rather
          # than iden3's stock linux archives (which do exist): swap_ffi is a
          # cdylib, and the static libs linked into a Linux shared object have
          # to be position-independent. These archives carry only `lib/`, which
          # is all RAPIDSNARK_LIB_DIR needs — no `bin/` or `include/` like the
          # macOS one. x86_64-darwin is absent for the same reason as
          # circuitsAsset above.
          rapidsnarkVersion = "v0.0.8";
          rapidsnarkAsset = {
            aarch64-darwin = {
              url = "https://github.com/iden3/rapidsnark/releases/download/${rapidsnarkVersion}/rapidsnark-macOS-arm64-${rapidsnarkVersion}.zip";
              sha256 = "1600dzr7hjg6lc5r0cdh189l7019djvy4cz2qyn75z5vrac4qs0f";
            };
            x86_64-linux = {
              url = "https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-${rapidsnarkVersion}/rapidsnark-linux-x86_64-pic-${rapidsnarkVersion}.zip";
              sha256 = "07qdnh4lm99alkmmg3av916bma7s86s616s56y0j4q4h82897kzk";
            };
            aarch64-linux = {
              url = "https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-${rapidsnarkVersion}/rapidsnark-linux-aarch64-pic-${rapidsnarkVersion}.zip";
              sha256 = "15f4iqy2szqpp84v8584s5b86vw8rfz60wx7h7ylp34r0m7qii4i";
            };
          }.${system} or (throw "no rapidsnark asset mapping for ${system}");
          rapidsnark = pkgs.fetchzip {
            inherit (rapidsnarkAsset) url sha256;
          };
          # LEZ v0.2.2 source (same commit as the Cargo.toml `tag = "v0.2.2"`
          # pins): its checked-in `artifacts/` tree (builtin program ELFs) is
          # copied into the cargo vendor dir below, because build_utils
          # resolves `../artifacts/...` relative to its own (vendored)
          # manifest dir via a compile-time env! — i.e. `<vendor>/artifacts`.
          lezSource = pkgs.fetchzip {
            url = "https://github.com/logos-blockchain/logos-execution-zone/archive/d6e4ae694e7419f5906b340c232704466a1917b7.tar.gz";
            sha256 = "07d04vbrj1vspnqjkr53iivywd53g5dvd4q5vrqhq9yqp6fjy3g1";
          };
          swapFfiSource = pkgs.runCommand "swap-ffi-source" {} ''
            cp -R ${swap-source}/. $out
            chmod -R u+w $out
          '';
        in {
          default = rustPlatform.buildRustPackage {
            pname = "swap-ffi";
            version = "0.1.0";

            src = swapFfiSource;
            # `cargoDeps` rather than `cargoHash` so vendoring goes through
            # the User-Agent-carrying fetcher above.
            #
            # `cargoHash`/`cargoDeps.hash` covers only the fixed-output
            # vendor-staging tree — the checksum-verified crate tarballs, git
            # checkouts and Cargo.lock — so it tracks the LOCKFILE, not the
            # nixpkgs building it (measured cold twice, against e9f00bd8 and
            # nixos-unstable 0954f7ee, 10 months and two different drvPaths
            # apart: identical `got:`). It DOES change whenever the workspace
            # Cargo.toml/Cargo.lock gains/drops a dependency — e.g. bumped
            # 2026-08-04 when the native-LEZ-faucet lift added `system_accounts`
            # + `getrandom` to the root Cargo.toml. Bump it here (and re-check
            # the whole repo for any other pinned `cargoHash`/`cargoDeps`/
            # `outputHash` — this is currently the only Rust vendor hash) any
            # time a workspace crate's dependency set changes; the build's own
            # error message gives you the correct value directly (`hash
            # mismatch ... use "sha256-..."`).
            #
            # Re-measure from a CLEAN checkout of the commit being pushed:
            # `swap-source` is a `path:".."` input, so it silently vendors
            # whatever lockfile happens to be on disk.
            cargoDeps = fetchCargoVendorUA {
              name = "swap-ffi-0.1.0";
              src = swapFfiSource;
              hash = "sha256-uZwBWt0kQZunoocu3ah84bpKp1cV/HWozNgf7JlnMBc=";
            };
            # --no-default-features: the `demo` feature only adds the risc0
            # guest build (needs the rzup toolchain + a nested cargo build the
            # sandbox can't run) for the program-ID drift test. The canonical
            # LEZ HTLC program ID itself ships as a checked-in constant
            # (swap-ffi/src/lez_htlc_program_id.rs), so the module still
            # surfaces it via swap_ffi_default_lez_htlc_program_id().
            cargoBuildFlags = [ "-p" "swap-ffi" "--no-default-features" ];
            doCheck = false;
            # (pyo3-ffi, which needed a python3 nativeBuildInput at the v0.2.0
            # pin, left the dependency graph with the v0.2.2 repin — no python
            # interpreter is needed by any build script anymore; verified by a
            # clean `nix build .#lib` without it.)
            LBC_ROOT_DIR = circuits;
            RAPIDSNARK_LIB_DIR = "${rapidsnark}/lib";

            postPatch = ''
              cp -R ${lezSource}/artifacts "$cargoDepsCopy/artifacts"
            '';

            installPhase = ''
              runHook preInstall

              mkdir -p $out/lib $out/include
              ffi_lib=$(find target -name ${dylibName} -print -quit)
              if [ -z "$ffi_lib" ]; then
                echo "swap-ffi build did not produce ${dylibName}" >&2
                exit 1
              fi
              cp "$ffi_lib" $out/lib/${dylibName}
              cp swap-ffi/include/swap_ffi.h $out/include/swap_ffi.h

              runHook postInstall
            '';

            postFixup = lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
              install_name_tool -id @rpath/${dylibName} $out/lib/${dylibName} || true
            '';
          };
        });
      swapFfiInput = {
        packages = swapFfiPackages;
      };

      base = logos-module-builder.lib.mkLogosModule {
        src = ./.;
        configFile = ./metadata.json;
        flakeInputs = inputs;
        externalLibInputs = {
          swap_ffi = {
            input = swapFfiInput;
            packages.default = "default";
          };
        };
        preConfigure = ''
          logos-cpp-generator --from-header src/swap_impl.h \
            --backend qt \
            --impl-class SwapImpl \
            --impl-header swap_impl.h \
            --metadata metadata.json \
            --output-dir ./generated_code
          substituteInPlace ./generated_code/swap_qt_glue.h \
            --replace '#include "swap_impl.h"' '#include "swap_impl.h"
#include "swap_delivery_adapter.h"' \
            --replace 'private:
    SwapImpl m_impl;' 'protected:
    void onInit(LogosAPI* api) override {
        swapDeliverySetRuntimeLogosAPI(static_cast<void*>(api));
    }

private:
    SwapImpl m_impl;'
          grep -q 'swap_delivery_adapter.h' ./generated_code/swap_qt_glue.h
          grep -q 'swapDeliverySetRuntimeLogosAPI' ./generated_code/swap_qt_glue.h
        '';
      };

      # Override the default dev shell so non-Nix dev iteration (ad-hoc CMake,
      # clangd, IDEs) can resolve the swap-ffi cdylib without a separate
      # `cargo build` + manual copy step. This replaces the retired
      # `make swap-vendor-ffi` Makefile target.
      devShellsWithSwapFfi = lib.genAttrs systems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          swapFfi = swapFfiPackages.${system}.default;
          dylibName = if pkgs.stdenv.hostPlatform.isDarwin
            then "libswap_ffi.dylib"
            else "libswap_ffi.so";
          libPathVar = if pkgs.stdenv.hostPlatform.isDarwin
            then "DYLD_LIBRARY_PATH"
            else "LD_LIBRARY_PATH";
          baseShell = base.devShells.${system}.default;
        in
        (base.devShells.${system} or {}) // {
          default = baseShell.overrideAttrs (old: {
            buildInputs = (old.buildInputs or []) ++ [ swapFfi ];
            shellHook = (old.shellHook or "") + ''
              # Stage the pre-built swap-ffi cdylib so the CMakeLists.txt
              # find_library(swap_ffi PATHS lib NO_DEFAULT_PATH) call resolves
              # it the same way it did under `make swap-vendor-ffi`. Only runs
              # when the shell is entered from the swap-module dir.
              if [ -f "$PWD/src/swap_impl.h" ] && [ -f "$PWD/metadata.json" ]; then
                mkdir -p "$PWD/lib"
                ln -sfn "${swapFfi}/lib/${dylibName}" "$PWD/lib/${dylibName}"
                export SWAP_FFI_LIB_DIR="$PWD/lib"
                echo "swap-ffi: staged ${swapFfi}/lib/${dylibName} -> $PWD/lib/${dylibName}"
              else
                echo "swap-ffi: skipped staging (run \`nix develop\` from the swap-module/ dir to auto-stage lib/${dylibName})"
              fi
              export ${libPathVar}="${swapFfi}/lib''${${libPathVar}:+:''$${libPathVar}}"
              export CMAKE_LIBRARY_PATH="${swapFfi}/lib''${CMAKE_LIBRARY_PATH:+:''$CMAKE_LIBRARY_PATH}"
              export CMAKE_INCLUDE_PATH="${swapFfi}/include''${CMAKE_INCLUDE_PATH:+:''$CMAKE_INCLUDE_PATH}"
              export CMAKE_EXPORT_COMPILE_COMMANDS=ON
            '';
          });
        });
    in
    base // { devShells = devShellsWithSwapFfi; };
}
