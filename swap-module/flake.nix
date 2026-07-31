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
    };
    nixpkgs.follows = "logos-module-builder/nixpkgs";
    # Used ONLY as the `follows` target above. Nothing in this flake's own
    # outputs is built from it, so it does not move the module's toolchain.
    nixpkgs-crates-io.url = "github:NixOS/nixpkgs/21ea275a7c46aef9d4d6ddc962e6d562e9d94183";
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
          # The substitution is asserted, so a nixpkgs bump that reshapes the
          # fetcher fails evaluation loudly instead of silently going back to
          # a 403. Delete all of this once logos-module-builder's nixpkgs
          # carries the upstream fix (logos-co/logos-module-builder#173).
          fetchCargoVendorUA =
            let
              rustBuildSupport = "${nixpkgs}/pkgs/build-support/rust";
              subst = from: to: text:
                let out = builtins.replaceStrings [ from ] [ to ] text; in
                if out == text
                then throw "swap-ffi: crates.io User-Agent workaround is stale — ${builtins.toJSON from} not found in nixpkgs' cargo vendor fetcher"
                else out;
              replaceWorkspaceValues = pkgs.writers.writePython3Bin "replace-workspace-values" {
                libraries = with pkgs.python3Packages; [ tomli tomli-w ];
                flakeIgnore = [ "E501" "W503" ];
              } (builtins.readFile "${rustBuildSupport}/replace-workspace-values.py");
              fetchCargoVendorUtil = pkgs.writers.writePython3Bin "fetch-cargo-vendor-util" {
                libraries = with pkgs.python3Packages; [ requests ];
                flakeIgnore = [ "E501" ];
              } (subst
                   "    session = requests.Session()\n"
                   "    session = requests.Session()\n    session.headers[\"User-Agent\"] = \"nixpkgs-fetchCargoVendor/1 (https://github.com/NixOS/nixpkgs)\"\n"
                   (builtins.readFile "${rustBuildSupport}/fetch-cargo-vendor-util.py"));
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
          # the LEZ v0.2.0 pin); their build scripts resolve it via
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
          # (pulled in at the v0.2.0 pin via logos-blockchain-circuits-prover,
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
          # LEZ v0.2.0 source (same commit as the Cargo.toml `tag = "v0.2.0"`
          # pins): its checked-in `artifacts/` tree (builtin program ELFs) is
          # copied into the cargo vendor dir below, because build_utils
          # resolves `../artifacts/...` relative to its own (vendored)
          # manifest dir via a compile-time env! — i.e. `<vendor>/artifacts`.
          lezSource = pkgs.fetchzip {
            url = "https://github.com/logos-blockchain/logos-execution-zone/archive/a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a.tar.gz";
            sha256 = "0f9vx32kx5y5cscnzb3xs6s9p2lsazs8fw1n1gpvxzn3g73w2x9s";
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
            # The value is the one master's `cargoHash` already carries.
            # `cargoHash` covers only the fixed-output vendor-staging tree —
            # the checksum-verified crate tarballs, git checkouts and
            # Cargo.lock — so it tracks the lockfile, not the nixpkgs
            # building it: measured cold twice, against e9f00bd8 and
            # nixos-unstable 0954f7ee (10 months and two different drvPaths
            # apart), and the `got:` was identical.
            #
            # Re-measure from a CLEAN checkout of the commit being pushed:
            # `swap-source` is a `path:".."` input, so it silently vendors
            # whatever lockfile happens to be on disk.
            cargoDeps = fetchCargoVendorUA {
              name = "swap-ffi-0.1.0";
              src = swapFfiSource;
              hash = "sha256-DePrHOfh7Ucu27FfPoQvn1Mu0StCtF6RcJJTEKqp2oA=";
            };
            # --no-default-features: the `demo` feature only adds the risc0
            # guest build (needs the rzup toolchain + a nested cargo build the
            # sandbox can't run) for the program-ID drift test. The canonical
            # LEZ HTLC program ID itself ships as a checked-in constant
            # (swap-ffi/src/lez_htlc_program_id.rs), so the module still
            # surfaces it via swap_ffi_default_lez_htlc_program_id().
            cargoBuildFlags = [ "-p" "swap-ffi" "--no-default-features" ];
            doCheck = false;
            # pyo3-ffi (transitive at the v0.2.0 pin) probes for a python3
            # interpreter in its build script.
            nativeBuildInputs = [ pkgs.python3 ];
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
