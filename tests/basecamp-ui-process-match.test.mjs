import assert from "node:assert/strict";
import test from "node:test";

import { moduleCommand } from "./basecamp-ui-process-match.mjs";

const row = (command) => ({ pid: 1, ppid: 0, pgid: 1, command });

test("accepts Basecamp wrapper and bundled ELF host names", () => {
  const cases = [
    ["/opt/basecamp/logos_host --name swap", "logos_host", "swap"],
    ["/opt/basecamp/.logos_host.elf --name swap", "logos_host", "swap"],
    ["/opt/basecamp/ui-host --name=swap_ui", "ui-host", "swap_ui"],
    ["/opt/basecamp/.ui-host.elf --name swap_ui", "ui-host", "swap_ui"],
  ];

  for (const [command, executable, moduleName] of cases) {
    assert.ok(moduleCommand([row(command)], executable, moduleName), command);
  }
});

test("rejects standalone, near-miss, and wrong-module processes", () => {
  const cases = [
    "/opt/basecamp/logos-standalone-app --name swap",
    "/opt/basecamp/evil.logos_host --name swap",
    "/opt/basecamp/logos_host_extra --name swap",
    "/opt/basecamp/.logos_host.elf.evil --name swap",
    "/opt/basecamp/logos_host --name swap_extra",
    "/opt/basecamp/logos_host --name delivery_module",
    "/opt/basecamp/ui-host --name swap",
  ];

  for (const command of cases) {
    assert.equal(moduleCommand([row(command)], "logos_host", "swap"), undefined, command);
  }
});

test("returns the matching row without accepting unrelated neighbors", () => {
  const expected = row("/opt/basecamp/.logos_host.elf --name delivery_module");
  const rows = [
    row("/opt/basecamp/logos-standalone-app --name delivery_module"),
    row("/opt/basecamp/logos_host_extra --name delivery_module"),
    expected,
  ];

  assert.equal(moduleCommand(rows, "logos_host", "delivery_module"), expected);
});
