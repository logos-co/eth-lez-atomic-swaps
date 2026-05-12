{
  description = "LEZ <> ETH atomic swap module (universal C++ wrapping swap-ffi)";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    nix-bundle-lgx.url = "github:logos-co/nix-bundle-lgx";
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
          swapFfiSource = pkgs.runCommand "swap-ffi-source-no-waku" {
            nativeBuildInputs = [ pkgs.python3 ];
          } ''
            cp -R ${swap-source}/. $out
            chmod -R u+w $out

            OUT="$out" python3 - <<'PY'
            import os
            from pathlib import Path

            root = Path(os.environ["OUT"])

            manifest_path = root / "Cargo.toml"
            manifest = manifest_path.read_text()
            for dep in ("waku-bindings", "multiaddr", "secp256k1", "rln"):
                manifest = "\n".join(
                    line for line in manifest.splitlines()
                    if not line.startswith(f"{dep} = ")
                )
            output_lines = []
            skipping_waku_feature = False
            for line in manifest.splitlines():
                if line == 'default = ["waku"]':
                    output_lines.append('default = []')
                elif line == 'waku = [':
                    output_lines.append('waku = []')
                    skipping_waku_feature = True
                elif skipping_waku_feature:
                    skipping_waku_feature = line != ']'
                elif line == '    "waku",':
                    continue
                else:
                    output_lines.append(line)
            manifest = "\n".join(output_lines)
            manifest_path.write_text(manifest + "\n")

            ffi_manifest_path = root / "swap-ffi" / "Cargo.toml"
            ffi_manifest = ffi_manifest_path.read_text()
            ffi_manifest = ffi_manifest.replace('default = ["waku"]', 'default = []')
            ffi_manifest_path.write_text(ffi_manifest)

            lock_path = root / "Cargo.lock"
            text = lock_path.read_text()
            blocks = text.split("\n[[package]]\n")
            kept = [blocks[0]]

            for block in blocks[1:]:
                if "github.com/logos-messaging/logos-delivery-rust-bindings" in block:
                    continue
                lines = [
                    line for line in block.splitlines()
                    if line not in {' "waku-bindings",', ' "waku-sys",'}
                ]
                kept.append("\n".join(lines))

            lock_path.write_text("\n[[package]]\n".join(kept))
            PY
          '';
        in {
          default = rustPlatform.buildRustPackage {
            pname = "swap-ffi";
            version = "0.1.0";

            src = swapFfiSource;
            cargoHash = "sha256-AFwbA/LU5nRm10SX8JHq0W70nrE+QMAjmzpP5UWc/mE=";
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
    in
    logos-module-builder.lib.mkLogosModule {
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
      '';
    };
}
