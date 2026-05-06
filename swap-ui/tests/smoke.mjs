// Integration smoke test for the swap-ui Basecamp app.
//
// IMPORTANT: `nix build .#integration-test` is currently broken upstream.
// `logos-standalone-app`'s mkPluginTest (the auto-wired runner from
// mkLogosQmlModule) launches the app with `-p <plugin-dir>` only and does
// NOT bundle module dependencies. Our `swap` core dep is consequently not
// loaded under that path, and the smoke test fails with "Module not found".
//
// To run this file manually against the apps.default runner (which DOES
// bundle deps via mkStandaloneApp.nix):
//
//   nix build .#test-framework -o result-mcp
//   nix run . -- --help >/dev/null
//   system=$(nix eval --raw --impure --expr builtins.currentSystem)
//   runner=$(nix eval --raw ".#apps.${system}.default.program")
//   LOGOS_QT_MCP=$(realpath result-mcp) QT_QPA_PLATFORM=offscreen \
//     node tests/smoke.mjs --ci "$runner" --verbose
//
// The smoke command launches Logos local IPC sockets. In agent sandboxes,
// run it with unsandboxed command permissions.
//
// Track upstream: when `mkPluginTest` accepts moduleDeps, this file becomes
// reachable via plain `nix build .#integration-test`.

import { resolve } from "node:path";

const root = process.env.LOGOS_QT_MCP
    || new URL("../result-mcp", import.meta.url).pathname;

const { test, run } = await import(
    resolve(root, "test-framework/framework.mjs")
);

test("swap_ui: loads UI and shows title", async (app) => {
    await app.waitFor(
        async () => { await app.expectTexts(["LEZ ↔ ETH Atomic Swap"]); },
        { timeout: 15000, interval: 500, description: "UI to load" }
    );
});

test("swap_ui: shows role buttons and primary action", async (app) => {
    await app.expectTexts(["Maker", "Taker", "Fetch Balances"]);
});

test("swap_ui: backend connects", async (app) => {
    await app.waitFor(
        async () => { await app.expectTexts(["Connected", "Status: Ready"]); },
        { timeout: 15000, interval: 500, description: "backend connection" }
    );
});

run();
