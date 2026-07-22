{
  description = "LEZ <> ETH atomic swap module (universal C++ wrapping swap-ffi)";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    nix-bundle-lgx.url = "github:logos-co/nix-bundle-lgx";
    delivery_module.url = "github:logos-co/logos-delivery-module/v0.1.1";
    nixpkgs.follows = "logos-module-builder/nixpkgs";
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
          # logos-blockchain-circuits prebuilt artifact. Version must match the
          # logos-blockchain-circuits-* crates in the workspace lock (v0.5.3 at
          # the LEZ v0.2.0 pin); their build scripts resolve it via
          # LBC_ROOT_DIR (the pre-0.5 LOGOS_BLOCKCHAIN_CIRCUITS env is gone)
          # and would otherwise try to download it inside the sandbox.
          circuitsPlatform = {
            aarch64-darwin = "macos-aarch64";
            x86_64-linux = "linux-x86_64";
            aarch64-linux = "linux-aarch64";
          }.${system} or (throw "logos-blockchain-circuits is not published for ${system}");
          circuitsHash = {
            aarch64-darwin = "0w3i0phgzjswsk1q2k6cr3001jjc55a82z79zw9w5p3x9hwaqljq";
          }.${system} or lib.fakeSha256;
          circuits = pkgs.fetchzip {
            url = "https://github.com/logos-blockchain/logos-blockchain-circuits/releases/download/v0.5.3/logos-blockchain-circuits-v0.5.3-${circuitsPlatform}.tar.gz";
            sha256 = circuitsHash;
          };
          # Prebuilt rapidsnark static libs for rust-rapidsnark's build.rs
          # (pulled in at the v0.2.0 pin via logos-blockchain-circuits-prover,
          # which forces the static-rapidsnark feature). Outside nix the build
          # script downloads these itself (see download_rapidsnark.sh in
          # logos-blockchain-rust-rapidsnark); the sandbox has no network, so
          # we prefetch the same per-target release asset and point
          # RAPIDSNARK_LIB_DIR at its lib/ dir.
          rapidsnarkVersion = "v0.0.8";
          rapidsnarkAsset = {
            aarch64-darwin = {
              url = "https://github.com/iden3/rapidsnark/releases/download/${rapidsnarkVersion}/rapidsnark-macOS-arm64-${rapidsnarkVersion}.zip";
              sha256 = "1600dzr7hjg6lc5r0cdh189l7019djvy4cz2qyn75z5vrac4qs0f";
            };
            x86_64-darwin = {
              url = "https://github.com/iden3/rapidsnark/releases/download/${rapidsnarkVersion}/rapidsnark-macOS-x86_64-${rapidsnarkVersion}.zip";
              sha256 = lib.fakeSha256;
            };
            x86_64-linux = {
              url = "https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-${rapidsnarkVersion}/rapidsnark-linux-x86_64-pic-${rapidsnarkVersion}.zip";
              sha256 = lib.fakeSha256;
            };
            aarch64-linux = {
              url = "https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-${rapidsnarkVersion}/rapidsnark-linux-aarch64-pic-${rapidsnarkVersion}.zip";
              sha256 = lib.fakeSha256;
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
            cargoHash = "sha256-Xu7WL+0fdJBMGDby0frPQtawBH8Gb/pmj+JXHc5LGC8=";
            # --no-default-features: the `demo` feature only adds the risc0
            # guest build (needs the rzup toolchain + a nested cargo build the
            # sandbox can't run) for the program-ID drift test. The canonical
            # LEZ HTLC program ID itself ships as a checked-in constant
            # (swap-ffi/src/lez_htlc_program_id.rs), so the module still
            # surfaces it via swap_ffi_default_lez_htlc_program_id().
            cargoBuildFlags = [ "-p" "swap-ffi" "--no-default-features" ];
            doCheck = false;
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
