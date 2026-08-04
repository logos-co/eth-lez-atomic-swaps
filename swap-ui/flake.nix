{
  description = "LEZ <> ETH atomic swap UI (Basecamp ui_qml app)";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    nix-bundle-lgx.url = "github:logos-co/nix-bundle-lgx";

    # Backend module — listed in metadata.json "dependencies" as "swap".
    # The flake input attribute name MUST match the dependency name.
    swap.url = "path:../swap-module";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    let
      base = logos-module-builder.lib.mkLogosQmlModule {
        src = ./.;
        configFile = ./metadata.json;
        flakeInputs = inputs;
      };
      packages = builtins.mapAttrs (_system: systemPackages:
        builtins.removeAttrs systemPackages [ "integration-test" "test-framework" ]
      ) base.packages;
      checks = builtins.mapAttrs (_system: systemChecks:
        builtins.removeAttrs systemChecks [ "integration-test" ]
      ) base.checks;
    in
    (builtins.removeAttrs base [ "apps" ]) // {
      inherit packages checks;
    };
}
