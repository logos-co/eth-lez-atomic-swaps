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
    in
    base // (
      if base ? apps then {
        apps = builtins.mapAttrs (_system: apps:
          apps // { app = apps.default; }
        ) base.apps;
      } else {}
    );
}
