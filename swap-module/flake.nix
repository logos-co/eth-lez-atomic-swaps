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
          circuitsPlatform = {
            aarch64-darwin = "macos-aarch64";
            x86_64-linux = "linux-x86_64";
            aarch64-linux = "linux-aarch64";
          }.${system} or (throw "logos-blockchain-circuits is not published for ${system}");
          circuitsHash = {
            aarch64-darwin = "1s5k0fmb8zic2pb03102vny9w1akqwzrp96ajmvzh9ylvdjbf7f0";
          }.${system} or lib.fakeSha256;
          circuits = pkgs.fetchzip {
            url = "https://github.com/logos-blockchain/logos-blockchain-circuits/releases/download/v0.4.2/logos-blockchain-circuits-v0.4.2-${circuitsPlatform}.tar.gz";
            sha256 = circuitsHash;
          };
          lssaSource = pkgs.fetchzip {
            url = "https://github.com/logos-blockchain/lssa/archive/9fa541f3d1cfa2d4415b7c3cf4cd8954a83b78c7.tar.gz";
            sha256 = "1ay400q0yd5rj80vgkpg1pinxh1jla2al5sdrnyyb7m43rkqn44a";
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
            cargoHash = "sha256-pR1e+m+V6rjNdyDVHcG33Fe+oWYjMR1BFvklG+dlTfo=";
            cargoBuildFlags = [ "-p" "swap-ffi" "--no-default-features" ];
            doCheck = false;
            LOGOS_BLOCKCHAIN_CIRCUITS = circuits;

            postPatch = ''
              mkdir -p "$cargoDepsCopy/artifacts"
              cp -R ${lssaSource}/artifacts/program_methods "$cargoDepsCopy/artifacts/program_methods"
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
