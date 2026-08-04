{
  description = "vacp2p/zerokit, re-exported with the rln package's checkPhase disabled";

  # See swap-module/flake.nix's `delivery_module` input comment (the block
  # that follows this flake in) for the full story. Summary (issue #70):
  #
  # zerokit's own flake (`nix/default.nix`) builds `packages.${system}.rln`
  # with `pkgs.rustPlatform.buildRustPackage`, which runs `cargo test -p rln`
  # in its default checkPhase (`doCheck` is never set, so it defaults to
  # true). `rln::public_api_tests::tree_test::test_leaf_setting_with_index`
  # (and its FFI twin, `test::test_leaf_setting_with_index_ffi`) is flaky on
  # linux-arm64: green on master 2026-08-03, red on PR #66 (docs-only) and
  # PR #69 (Rust-only), green again on the 0.3.2 release run. Confirmed
  # flake, not a deterministic break (see issue #70 and its comments) — but
  # it has reddened 2 of the last 3 PRs, neither of which could plausibly
  # have caused it, and the nightly canary (all three variants as of PR #65)
  # and both release workflows inherit the same flake.
  #
  # We assert nothing about our own code by running zerokit's RLN
  # merkle-tree tests, so this re-export turns that checkPhase off. It is
  # deliberately NOT a fork of zerokit or logos-delivery: nothing here
  # patches either repo's source, it only changes which build attribute
  # (`doCheck`) is used when we ourselves build the dependency, via the same
  # `follows` mechanism already used for the nixpkgs-crates-io fix next to
  # this input in swap-module/flake.nix.
  #
  # `doCheck = false` changes the `rln` derivation itself (it is part of the
  # build script, not the fixed-output vendor step), so this DOES change the
  # derivation's store path and forces a rebuild of everything downstream
  # (logos-delivery -> delivery_module -> swap-module -> swap-ui). It does
  # NOT touch zerokit's committed `cargoHash` (that covers only the vendored
  # Cargo.lock, which is unaffected by checkPhase) and it does NOT touch this
  # repo's own `cargoHash`/`cargoDeps` for swap-ffi.
  #
  # File a real upstream report on the underlying bug (`MerkleTree(InvalidLeaf)`
  # on aarch64 Linux, both lib and FFI paths) against vacp2p/zerokit; this
  # workaround does not fix that, it just stops running someone else's test
  # suite inside our build graph. Delete this once zerokit's own CI is green
  # on linux-arm64 without gaps, or once it exposes a `doCheck` toggle itself.
  inputs.zerokit.url = "github:vacp2p/zerokit/53b18098e6d5d046e3eb1ac338a8f4f651432477";

  outputs = { self, zerokit }: {
    packages = builtins.mapAttrs
      (system: pkgs:
        pkgs // {
          rln = pkgs.rln.overrideAttrs (_old: { doCheck = false; });
        })
      zerokit.packages;
  };
}
