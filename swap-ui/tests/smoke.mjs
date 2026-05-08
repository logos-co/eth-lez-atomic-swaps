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

test("swap_ui: loads UI and shows primary chrome", async (app) => {
    await app.waitFor(
        async () => { await app.expectTexts(["LEZ Atomic Swap", "Configuration"]); },
        { timeout: 15000, interval: 500, description: "UI to load" }
    );
});

test("swap_ui: shows config panel", async (app) => {
    await app.expectTexts([
        "Configuration",
        "Load Maker Env",
        "Load Taker Env",
        "Ethereum",
        "LEZ",
        "Swap Parameters",
        "Messaging"
    ]);
});

test("swap_ui: backend connects", async (app) => {
    await app.waitFor(
        async () => { await app.expectTexts(["Please choose a configuration."]); },
        { timeout: 15000, interval: 500, description: "backend connection" }
    );
});

test("swap_ui: validation errors are surfaced", async (app) => {
    await app.waitFor(
        async () => { await app.expectTexts(["Required"]); },
        { timeout: 5000, interval: 250, description: "validation field error text" }
    );
});

test("swap_ui: primary action labels are present", async (app) => {
    await app.expectTexts(["Refresh", "Load Maker Env", "Load Taker Env"]);
});

test("swap_ui: live offer copy is present", async (app) => {
    await app.click("Maker");
    await app.expectTexts([
        "Go Live & Publish Offer",
        "Publishes your current rate as an actionable offer"
    ]);

    await app.click("Taker");
    await app.expectTexts(["Offers are advertisements"]);
});

run();
