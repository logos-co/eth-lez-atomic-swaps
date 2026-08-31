#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { createServer } from "node:net";
import { createWriteStream, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { basename, isAbsolute, join, resolve } from "node:path";
import { moduleCommand } from "./basecamp-ui-process-match.mjs";

const appBin = process.env.BASECAMP_BIN;
const userDir = process.env.BASECAMP_USER_DIR;
const runtimeDir = process.env.BASECAMP_RUNTIME_DIR;
const artifactDir = resolve(process.env.BASECAMP_UI_ARTIFACT_DIR || "artifacts/basecamp-ui-smoke");
const qtMcpRoot = process.env.LOGOS_QT_MCP;

for (const [name, value] of Object.entries({ appBin, userDir, runtimeDir, qtMcpRoot })) {
  if (!value) throw new Error(`missing required environment variable for ${name}`);
}
if (basename(appBin) !== "LogosBasecamp")
  throw new Error(`refusing to launch a non-Basecamp host: ${appBin}`);
for (const [name, value] of Object.entries({ userDir, runtimeDir, artifactDir })) {
  if (!isAbsolute(value)) throw new Error(`${name} must be absolute: ${value}`);
}
if (!/^\/tmp\/atomic-swaps-basecamp-ui\.[A-Za-z0-9]+\/user-dir$/.test(userDir)) {
  throw new Error(`refusing to use a non-ephemeral Basecamp user-dir: ${userDir}`);
}

mkdirSync(artifactDir, { recursive: true });

async function reserveInspectorPort() {
  const server = createServer();
  await new Promise((resolveListen, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  await new Promise((resolveClose, reject) => server.close((error) => error ? reject(error) : resolveClose()));
  if (!port) throw new Error("failed to reserve a QML inspector port");
  return port;
}

function processRows() {
  const output = execFileSync("ps", ["-axo", "pid=,ppid=,pgid=,command="], { encoding: "utf8" });
  return output.split("\n").flatMap((line) => {
    const match = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/);
    return match ? [{ pid: Number(match[1]), ppid: Number(match[2]), pgid: Number(match[3]), command: match[4] }] : [];
  });
}

function descendantsOf(rootPid) {
  const rows = processRows();
  const found = [];
  const queue = [rootPid];
  while (queue.length > 0) {
    const parent = queue.shift();
    for (const row of rows) {
      if (row.ppid === parent && !found.some((entry) => entry.pid === row.pid)) {
        found.push(row);
        queue.push(row.pid);
      }
    }
  }
  return found;
}

function formatProcessTree(rootPid) {
  const descendantPids = new Set(descendantsOf(rootPid).map((row) => row.pid));
  const rows = processRows().filter((row) => row.pid === rootPid || descendantPids.has(row.pid));
  return rows.map((row) => `${row.pid}\t${row.ppid}\t${row.pgid}\t${row.command}`).join("\n") + "\n";
}

class FatalWaitError extends Error {}

async function waitFor(description, callback, timeoutMs = 45000, intervalMs = 500) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await callback();
      if (value) return value;
    } catch (error) {
      if (error instanceof FatalWaitError) throw error;
      lastError = error;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, intervalMs));
  }
  throw new Error(`${description} did not become ready within ${timeoutMs}ms${lastError ? `: ${lastError.message}` : ""}`);
}

function capturedSurvivors(captured) {
  const currentByPid = new Map(processRows().map((row) => [row.pid, row]));
  return captured.flatMap((original) => {
    const current = currentByPid.get(original.pid);
    return current ? [{ original, current }] : [];
  });
}

function verifiedOwnedSurvivors(captured) {
  const survivors = capturedSurvivors(captured);
  const unsafe = survivors.filter(({ original, current }) =>
    current.command !== original.command || !current.command.includes(userDir)
  );
  if (unsafe.length > 0) {
    throw new Error(
      "refusing to signal a surviving PID whose command no longer matches the captured Basecamp test process:\n" +
      unsafe.map(({ original, current }) =>
        `${original.pid}\n  captured: ${original.command}\n  current:  ${current.command}`
      ).join("\n")
    );
  }
  return survivors.map(({ current }) => current);
}

async function waitForChildExit(child, timeoutMs) {
  if (child.exitCode !== null || child.signalCode !== null) return true;
  return Promise.race([
    new Promise((resolveExit) => child.once("exit", () => resolveExit(true))),
    new Promise((resolveTimeout) => setTimeout(() => resolveTimeout(false), timeoutMs)),
  ]);
}

async function terminateOwnedProcesses(child) {
  if (!child?.pid) return;
  const pgid = child.pid; // spawn({ detached: true }) creates this new POSIX process group.
  const rows = processRows();
  const root = rows.find((row) => row.pid === child.pid);
  // The unique absolute mktemp user-dir is the durable ownership boundary.
  // Ancestry is useful while Basecamp is alive, but module hosts can be
  // reparented if the root crashes before teardown begins.
  const captured = rows.filter((row) => row.command.includes(userDir));
  if (captured.length === 0) return;

  if (root?.command.includes(userDir)) {
    try { process.kill(-pgid, "SIGTERM"); } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
    await waitForChildExit(child, 8000);
  }
  await waitFor("Basecamp process tree to stop after SIGTERM", () => capturedSurvivors(captured).length === 0, 8000, 250)
    .catch(() => false);

  // Basecamp intentionally starts each logos_host/ui-host in its own process
  // group. Normal SIGTERM shutdown reaps them. If one survives, signal only
  // the exact captured PID after verifying its full command is unchanged and
  // still contains this run's unique user-dir marker.
  let survivors = verifiedOwnedSurvivors(captured);
  for (const survivor of survivors) {
    try { process.kill(survivor.pid, "SIGTERM"); } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
  if (survivors.length > 0) {
    await waitFor("captured Basecamp children to stop", () => capturedSurvivors(captured).length === 0, 3000, 250)
      .catch(() => false);
  }

  survivors = verifiedOwnedSurvivors(captured);
  for (const survivor of survivors) {
    try { process.kill(survivor.pid, "SIGKILL"); } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
  if (survivors.length > 0) {
    await new Promise((resolveWait) => setTimeout(resolveWait, 500));
  }

  survivors = capturedSurvivors(captured).map(({ current }) => current);
  if (survivors.length > 0) {
    throw new Error(`captured Basecamp processes survived teardown:\n${survivors.map((row) => row.command).join("\n")}`);
  }
}

async function saveInspectorArtifacts(app, prefix) {
  const safeWriteJson = async (name, callback) => {
    try {
      const value = await callback();
      writeFileSync(join(artifactDir, `${prefix}-${name}.json`), JSON.stringify(value, null, 2));
    } catch (error) {
      writeFileSync(join(artifactDir, `${prefix}-${name}.error.txt`), `${error.stack || error}\n`);
    }
  };

  try {
    const screenshot = await app.screenshot();
    if (screenshot?.image) {
      writeFileSync(join(artifactDir, `${prefix}.png`), Buffer.from(screenshot.image, "base64"));
    } else {
      writeFileSync(join(artifactDir, `${prefix}-screenshot.error.txt`), JSON.stringify(screenshot, null, 2));
    }
  } catch (error) {
    writeFileSync(join(artifactDir, `${prefix}-screenshot.error.txt`), `${error.stack || error}\n`);
  }
  await safeWriteJson("tree", () => app.getTree({ depth: 40 }));
  await safeWriteJson("interactive", () => app.listInteractive());
}

const inspectorPort = await reserveInspectorPort();
process.env.QML_INSPECTOR_HOST = "127.0.0.1";
process.env.QML_INSPECTOR_PORT = String(inspectorPort);
const { App, Inspector } = await import(resolve(qtMcpRoot, "test-framework/framework.mjs"));

const processLog = createWriteStream(join(artifactDir, "basecamp-process.log"), { flags: "w" });
const child = spawn(appBin, ["--user-dir", userDir, "-platform", "offscreen"], {
  detached: true,
  stdio: ["ignore", "pipe", "pipe"],
  env: {
    ...process.env,
    LOGOS_USER_DIR: userDir,
    XDG_RUNTIME_DIR: runtimeDir,
    TMPDIR: runtimeDir,
    QML_INSPECTOR_HOST: "127.0.0.1",
    QML_INSPECTOR_PORT: String(inspectorPort),
    QT_QPA_PLATFORM: "offscreen",
    QT_FORCE_STDERR_LOGGING: "1",
    QT_LOGGING_RULES: "qt.*.debug=false;default.debug=true",
  },
});
child.stdout.pipe(processLog, { end: false });
child.stderr.pipe(processLog, { end: false });
let spawnError;
child.once("error", (error) => { spawnError = error; });

let inspector;
let app;
let failure;
let teardownStarted = false;

async function teardown() {
  if (teardownStarted) return;
  teardownStarted = true;
  inspector?.disconnect();
  await terminateOwnedProcesses(child);
  await new Promise((resolveEnd) => processLog.end(resolveEnd));
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, () => {
    teardown().finally(() => process.exit(signal === "SIGINT" ? 130 : 143));
  });
}

try {
  inspector = await waitFor("Basecamp QML inspector", async () => {
    if (spawnError) throw new FatalWaitError(`Basecamp failed to launch: ${spawnError.message}`);
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new FatalWaitError(`Basecamp exited early (code=${child.exitCode}, signal=${child.signalCode})`);
    }
    const candidate = new Inspector();
    await candidate.connect();
    return candidate;
  // Match Basecamp's own headless doc-test launch budget. A cold portable
  // macOS bundle can spend more than a minute relocating/starting its embedded
  // package-manager modules before MainWindow attaches the inspector.
  }, 300000, 500);
  app = new App(inspector);

  await app.waitFor(
    () => app.expectTexts(["ETH ↔ LEZ Atomic Swap"]),
    { timeout: 30000, interval: 500, description: "Atomic Swaps launcher entry" },
  );
  await app.click("ETH ↔ LEZ Atomic Swap", { exact: true });
  await app.waitFor(
    () => app.expectTexts(["Market", "Swap", "History", "Sell", "Refund", "Setup"]),
    { timeout: 30000, interval: 500, description: "swap_ui view" },
  );

  const topology = await waitFor("Basecamp module process topology", () => {
    const rows = descendantsOf(child.pid);
    const forbidden = rows.find((row) => row.command.includes("logos-standalone-app"));
    if (forbidden) throw new Error(`unexpected module-owned app host: ${forbidden.command}`);
    const delivery = moduleCommand(rows, "logos_host", "delivery_module");
    const swap = moduleCommand(rows, "logos_host", "swap");
    const ui = moduleCommand(rows, "ui-host", "swap_ui");
    return delivery && swap && ui ? { rows, delivery, swap, ui } : null;
  });
  writeFileSync(join(artifactDir, "process-tree.txt"), formatProcessTree(child.pid));

  await app.waitFor(async () => {
    const connecting = await app.findByProperty("text", "Connecting to backend...");
    if (connecting.matches?.length) throw new Error("swap_ui backend is still connecting");
  }, { timeout: 30000, interval: 500, description: "swap_ui backend connection" });

  const tabChecks = [
    ["Market",  ["LIVE MARKET"]],
    ["Swap",    ["Your swap"]],
    ["Sell",    ["Sell LEZ"]],
    ["Refund",  ["Get your money back"]],
    ["History", ["Swap history"]],
    ["Setup",   ["Get set up"]],
  ];
  for (const [tab, texts] of tabChecks) {
    await app.click(tab, { exact: true });
    await app.waitFor(
      () => app.expectTexts(texts),
      { timeout: 10000, interval: 300, description: `${tab} tab` },
    );
  }

  // Delivery-context creation must not fail. swap_ui auto-starts messaging at
  // load (SwapUiPlugin::startBackgroundServices), so a config the delivery
  // module rejects surfaces here as a createNode failure in the captured host
  // log. The delivery plugin reports it via qWarning ("Failed to create
  // Delivery context") — shown by default — so a negative assertion on the log
  // is reliable regardless of qDebug filtering. This closes the gap that let
  // PR #120 go green while delivery_module v0.1.1 rejected our config with
  // "Unrecognized configuration option(s) found: [numShardsInCluster,
  // messagingOverrides]" and every startup delivery context creation failed.
  const deliveryLog = join(artifactDir, "basecamp-process.log");
  const deliveryFailureMarkers = [
    "Failed to create Delivery context",
    "Unrecognized configuration option",
    "createNode callback error",
  ];
  const deliverySuccessMarker = "Delivery context created successfully";
  const deliveryDeadline = Date.now() + 20000;
  let deliveryLogText = "";
  let deliveryFailure;
  let deliverySucceeded = false;
  while (Date.now() < deliveryDeadline) {
    try { deliveryLogText = readFileSync(deliveryLog, "utf8"); } catch { deliveryLogText = ""; }
    deliveryFailure = deliveryFailureMarkers.find((m) => deliveryLogText.includes(m));
    deliverySucceeded = deliveryLogText.includes(deliverySuccessMarker);
    if (deliveryFailure || deliverySucceeded) break;
    await new Promise((r) => setTimeout(r, 500));
  }
  if (deliveryFailure) {
    throw new Error(
      `delivery_module createNode failed in the host log (marker: "${deliveryFailure}") — ` +
      `the config handed to createNode was rejected. See ${deliveryLog}.`,
    );
  }
  // Positive proof is best-effort: the success line is qDebug and may be
  // filtered out of a release build's log, so its absence is only a soft signal
  // (the hard gate above is the reliable one).
  console.log(deliverySucceeded
    ? "delivery_module createNode succeeded (host log)"
    : "delivery_module createNode: no failure marker (success marker is qDebug, may be filtered)");

  // ── End-to-end offer RECEPTION ─────────────────────────────────────────────
  // A created delivery context is necessary but nowhere near sufficient: the
  // 2026-08-10 incident had the node subscribed on cluster-3 shard-7,
  // receiving and re-relaying the real maker's offers, while the swap
  // adapter silently dropped every one of them (it parsed delivery_module
  // v0.2.0's raw-QByteArray messageReceived payload with the v0.1.x
  // base64-QString contract). This assertion closes that entire bug class:
  // the swap adapter prints the exact marker below (fprintf(stderr), so it is
  // immune to Qt log filtering) only after a fleet offer survived the full
  // path — cluster-3 mesh -> delivery_module v0.2.0 typed messageReceived
  // event -> adapter payload decode -> offer-field validation -> offer cache
  // (the store fetchOffers serves the UI from). The real VPS maker publishes
  // an offer on /atomic-swaps/1/offers/json every 30s, so the budget below
  // covers two heartbeats plus cold gossipsub mesh formation.
  const receptionMarker = "offer cached from delivery";
  const receptionDropMarker = "SwapDeliveryAdapter: dropped offers-topic payload";
  const receptionTimeoutMs = Number(process.env.BASECAMP_UI_RECEPTION_TIMEOUT_MS || 150000);
  const readHostLogs = () => {
    let text = "";
    try { text += readFileSync(deliveryLog, "utf8"); } catch {}
    // Module hosts inherit Basecamp's stderr (that is how the delivery
    // markers above land in basecamp-process.log), but also sweep any
    // per-module log files Basecamp writes under the user dir, defensively.
    const logsDir = join(userDir, "logs");
    try {
      for (const entry of readdirSync(logsDir, { recursive: true })) {
        const file = join(logsDir, String(entry));
        try {
          if (statSync(file).isFile()) text += readFileSync(file, "utf8");
        } catch {}
      }
    } catch {}
    return text;
  };
  const receptionDeadline = Date.now() + receptionTimeoutMs;
  let receptionLogText = "";
  let offerReceived = false;
  while (Date.now() < receptionDeadline) {
    receptionLogText = readHostLogs();
    if (receptionLogText.includes(receptionMarker)) { offerReceived = true; break; }
    await new Promise((r) => setTimeout(r, 2000));
  }
  if (!offerReceived) {
    // Fail loudly, and say which leg of the path broke so the log dump is
    // not the starting point of the investigation.
    const saw = (needle) => receptionLogText.includes(needle);
    const diagnostics = [
      `delivery context created: ${saw("Delivery context created successfully")}`,
      `subscribe completed (delivery log): ${saw("Subscribe completed for topic: /atomic-swaps/1/offers/json")}`,
      `relay message received (node log): ${saw("received relay message")}`,
      `messageReceived event reached module (delivery log): ${saw('"eventType":"message_received"')}`,
      `adapter dropped an offers-topic payload: ${saw(receptionDropMarker)}`,
    ];
    throw new Error(
      `no offer was received end-to-end within ${receptionTimeoutMs}ms — ` +
      `the marker "${receptionMarker}" never appeared in the host logs. ` +
      `The real fleet maker publishes every 30s, so this means the ` +
      `fleet mesh, the delivery_module event path, or the swap adapter is broken.\n` +
      `Diagnostics:\n  ${diagnostics.join("\n  ")}\nSee ${deliveryLog}.`,
    );
  }
  console.log("end-to-end offer reception verified (offer cached from delivery, host log)");

  await saveInspectorArtifacts(app, "success");
  console.log("Basecamp-native swap_ui smoke test passed");
} catch (error) {
  failure = error;
  writeFileSync(join(artifactDir, "failure.txt"), `${error.stack || error}\n`);
  if (child?.pid) {
    try { writeFileSync(join(artifactDir, "failure-process-tree.txt"), formatProcessTree(child.pid)); } catch {}
  }
  if (app) await saveInspectorArtifacts(app, "failure");
} finally {
  try {
    await teardown();
  } catch (error) {
    failure ||= error;
    writeFileSync(join(artifactDir, "teardown-failure.txt"), `${error.stack || error}\n`);
  }
}

if (failure) throw failure;
