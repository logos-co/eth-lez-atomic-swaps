# LSP Setup

Install the recommended VS Code or Cursor extensions:

- `rust-lang.rust-analyzer`
- `llvm-vs-code-extensions.vscode-clangd`
- `ms-vscode.cmake-tools`
- `theqtcompany.qt-qml`
- `juanblanco.solidity`
- `jnoortheen.nix-ide`
- `tamasfe.even-better-toml`
- `redhat.vscode-yaml`
- `davidanson.vscode-markdownlint`
- `ms-vscode.makefile-tools`

For Rust, open the repo root so rust-analyzer picks up the linked Cargo projects in `.vscode/settings.json`.

For C++ and Qt, generate `build/compile_commands.json` with the Logos module builder environment available when needed, for example:

```sh
LOGOS_MODULE_BUILDER_ROOT=/path/to/logos-module-builder cmake -S swap-module -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
```

`clangd` reads that database from `build/` and falls back to minimal C++17 flags plus local `swap-module` and `swap-ui` include paths from `.clangd`.

QML editing works best with `TheQtCompany.qt-qml` and Qt 6.8+ language server support, or with a separately configured `qmlls`.

Solidity in `contracts/` uses Foundry configuration from `contracts/foundry.toml`.

Nix files use `jnoortheen.nix-ide` with `nil` installed or another configured Nix language server.

## OpenCode

OpenCode/Oh My OpenAgent uses `.opencode/oh-my-openagent.json` instead of VS Code settings. This repo enables the language servers currently expected on PATH:

- `rust-analyzer` for Rust (`.rs`)
- `clangd` for C and C++ (`.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hh`, `.hpp`, `.hxx`)
- `qmlls` for QML (`.qml`)

Restart OpenCode after changing `.opencode/oh-my-openagent.json` so it reloads the LSP registry.

If Rust diagnostics fail with `Unknown binary 'rust-analyzer' in official toolchain`, install the component for the pinned toolchain:

```sh
rustup component add rust-analyzer --toolchain 1.93.0
```

If C++ diagnostics cannot find Qt or generated module headers, generate `build/compile_commands.json` first as described above.

Optional servers such as `nil` or `nixd` for Nix, Markdown servers, YAML servers, and JSON servers are not enabled here because they are not required by the repo and may not be installed locally. Add them to `.opencode/oh-my-openagent.json` after installing the corresponding binaries.
