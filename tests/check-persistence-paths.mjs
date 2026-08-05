import assert from "node:assert/strict"
import { readFileSync } from "node:fs"

// Source-contract test for the issue-#99 per-profile persistence fix.
//
// swap_ui is a ui_qml plugin hosted in an out-of-process ui-host that gets no
// LOGOS_USER_DIR under a default Basecamp launch, so its Qt AppDataLocation
// fallback lands in a `Logos/ui-host/` tree SHARED across every profile —
// leaking config.json's two private keys (eth_private_key, lez_signing_key).
// The fix anchors writes to a per-profile directory the swap CORE module
// reports (its host-provisioned instancePersistencePath()), and keeps the old
// shared path as a READ-ONLY migration fallback that is never written.
//
// The path logic is Qt/C++ (QDir, QStandardPaths, qEnvironmentVariable) and
// cannot be executed under Node, so — mirroring check-feedback-evidence.mjs —
// this asserts the structural contract on the source: which helper each write
// path goes through, that the shared tree is referenced from exactly one
// read-only place, and that the late-arriving root triggers a reload.

function source(path) {
  return readFileSync(new URL(`../${path}`, import.meta.url), "utf8")
}

// Extract a C++ method body ("{ ... }") by qualified name, brace-matching while
// skipping string/char literals and // and /* */ comments (any of which may
// legitimately contain braces).
function cppBody(text, qualifiedName) {
  const at = text.indexOf(qualifiedName + "(")
  assert.notEqual(at, -1, `missing ${qualifiedName}`)
  const paren = text.indexOf(")", at)
  assert.notEqual(paren, -1, `malformed signature for ${qualifiedName}`)
  const open = text.indexOf("{", paren)
  assert.notEqual(open, -1, `no body for ${qualifiedName}`)
  let depth = 0
  let i = open
  while (i < text.length) {
    const c = text[i]
    const c2 = text[i + 1]
    if (c === "/" && c2 === "/") {
      const nl = text.indexOf("\n", i)
      i = nl === -1 ? text.length : nl
      continue
    }
    if (c === "/" && c2 === "*") {
      const end = text.indexOf("*/", i + 2)
      i = end === -1 ? text.length : end + 2
      continue
    }
    if (c === '"' || c === "'") {
      i += 1
      while (i < text.length) {
        if (text[i] === "\\") { i += 2; continue }
        if (text[i] === c) { i += 1; break }
        i += 1
      }
      continue
    }
    if (c === "{") depth += 1
    else if (c === "}" && --depth === 0) return text.slice(open, i + 1)
    i += 1
  }
  assert.fail(`unterminated body for ${qualifiedName}`)
}

const cpp = source("swap-ui/src/swap_ui_plugin.cpp")
const hdr = source("swap-ui/src/swap_ui_plugin.h")
const implHdr = source("swap-module/src/swap_impl.h")
const implCpp = source("swap-module/src/swap_impl.cpp")

// ---------------------------------------------------------------------------
// The shared ui-host tree (Qt AppDataLocation) is referenced from exactly one
// place, and that place is the READ-ONLY legacy helper. This is the core
// guarantee: no write path can reach the shared tree.
// ---------------------------------------------------------------------------
const appDataRefs = [...cpp.matchAll(/QStandardPaths::AppDataLocation/g)]
assert.equal(appDataRefs.length, 1,
  "Qt AppDataLocation (the shared ui-host tree) must be referenced exactly once")

const legacyDir = cppBody(cpp, "SwapUiPlugin::legacyModuleDir")
assert.match(legacyDir, /AppDataLocation/,
  "legacyModuleDir() is the sole AppDataLocation resolver")
assert.match(legacyDir, /module_data\/swap_ui/,
  "legacyModuleDir() must point at the historical module_data/swap_ui path")

// ---------------------------------------------------------------------------
// writableModuleDir(): (a) swap-core per-profile root, then (b) LOGOS_USER_DIR;
// empty when neither is known — and NEVER the shared tree.
// ---------------------------------------------------------------------------
const writableDir = cppBody(cpp, "SwapUiPlugin::writableModuleDir")
assert.match(writableDir, /m_persistenceRoot/,
  "writableModuleDir() must prefer the swap-core per-profile root")
assert.match(writableDir, /"swap_ui"/,
  "writableModuleDir() anchors core-provided writes under a swap_ui subdir")
assert.match(writableDir, /LOGOS_USER_DIR/,
  "writableModuleDir() must accept an explicit LOGOS_USER_DIR launch")
assert.match(writableDir, /return \{\}/,
  "writableModuleDir() must return empty when no per-profile location is known")
assert.doesNotMatch(writableDir, /AppDataLocation/,
  "writableModuleDir() must never resolve to the shared ui-host tree")

// ---------------------------------------------------------------------------
// Reads prefer the per-profile copy, then fall back (once) to the legacy file.
// ---------------------------------------------------------------------------
for (const name of ["configReadPath", "receiptsReadPath"]) {
  const body = cppBody(cpp, `SwapUiPlugin::${name}`)
  assert.match(body, /writableModuleDir\(\)/, `${name}() prefers the writable dir`)
  assert.match(body, /QFileInfo::exists/, `${name}() only prefers writable when it exists`)
  assert.match(body, /legacyModuleDir\(\)/, `${name}() falls back to the legacy file`)
}

// ---------------------------------------------------------------------------
// Writers go through writableModuleDir() and defer/buffer (never the shared
// tree) when the per-profile root is not yet known.
// ---------------------------------------------------------------------------
const saveConfig = cppBody(cpp, "SwapUiPlugin::saveConfigToDisk")
assert.match(saveConfig, /writableModuleDir\(\)/, "saveConfigToDisk() writes to the writable dir")
assert.match(saveConfig, /baseDir\.isEmpty\(\)/, "saveConfigToDisk() guards on an unresolved root")
assert.match(saveConfig, /m_pendingConfigSave = true/, "saveConfigToDisk() defers when unresolved")
assert.doesNotMatch(saveConfig, /legacyModuleDir|AppDataLocation/,
  "saveConfigToDisk() must never write to the legacy shared tree")
// 0600 permissions from #89 preserved.
assert.match(saveConfig, /ReadOwner \| QFileDevice::WriteOwner/,
  "saveConfigToDisk() must keep the 0600 permissions for the private-key file")

const journal = cppBody(cpp, "SwapUiPlugin::journalReceipt")
assert.match(journal, /writableModuleDir\(\)/, "journalReceipt() writes to the writable dir")
assert.match(journal, /m_pendingReceiptLines\.append/, "journalReceipt() buffers when unresolved")
assert.doesNotMatch(journal, /legacyModuleDir|AppDataLocation/,
  "journalReceipt() must never append to the legacy shared tree")

// ---------------------------------------------------------------------------
// Destructive user actions only ever touch the per-profile file — never the
// shared legacy file (Basecamp #315/#316 gate: old file cleaned up by hand).
// ---------------------------------------------------------------------------
for (const name of ["resetConfig", "clearHistory"]) {
  const body = cppBody(cpp, `SwapUiPlugin::${name}`)
  assert.match(body, /writableModuleDir\(\)/, `${name}() operates on the writable dir`)
  assert.doesNotMatch(body, /legacyModuleDir|AppDataLocation/,
    `${name}() must never delete/modify the legacy shared file`)
}

// ---------------------------------------------------------------------------
// Loaders read via the *ReadPath() helpers (migration-read aware).
// ---------------------------------------------------------------------------
assert.match(cppBody(cpp, "SwapUiPlugin::loadConfigFromDisk"), /configReadPath\(\)/,
  "loadConfigFromDisk() reads via configReadPath()")
assert.match(cppBody(cpp, "SwapUiPlugin::loadReceiptsFromDisk"), /receiptsReadPath\(\)/,
  "loadReceiptsFromDisk() reads via receiptsReadPath()")
// No stale references to the removed absolute-path helpers.
assert.doesNotMatch(cpp, /\bconfigFilePath\b/, "configFilePath() must be fully removed")
assert.doesNotMatch(cpp, /\breceiptsFilePath\b/, "receiptsFilePath() must be fully removed")

// ---------------------------------------------------------------------------
// Late-arriving per-profile root: requested at init, applied via a reload.
// ---------------------------------------------------------------------------
assert.match(cppBody(cpp, "SwapUiPlugin::initLogos"), /requestPersistenceRoot\(\)/,
  "initLogos() must request the per-profile root from the swap core module")

const request = cppBody(cpp, "SwapUiPlugin::requestPersistenceRoot")
assert.match(request, /m_swap->persistenceRootAsync/,
  "requestPersistenceRoot() must query the swap core module's persistenceRoot")
assert.match(request, /onPersistenceRootResolved/,
  "requestPersistenceRoot() must route the result to onPersistenceRootResolved()")

const resolved = cppBody(cpp, "SwapUiPlugin::onPersistenceRootResolved")
assert.match(resolved, /m_persistenceRoot = root/,
  "onPersistenceRootResolved() must adopt the reported root")
assert.match(resolved, /loadConfigFromDisk\(\)/,
  "onPersistenceRootResolved() must reload config once the root arrives")
assert.match(resolved, /loadReceiptsFromDisk\(\)/,
  "onPersistenceRootResolved() must reload receipts once the root arrives")
assert.match(resolved, /flushPendingReceipts\(\)/,
  "onPersistenceRootResolved() must flush any buffered receipts")

// ---------------------------------------------------------------------------
// Release-verification marker: a distinctive literal that must survive into the
// built swap_ui .lgx (asserted by the release pipeline). Keep in sync with the
// PR body and the release grep.
// ---------------------------------------------------------------------------
const MARKER = "swap_ui persistence root (per-profile)"
assert.ok(cpp.includes(MARKER), `the release-verification marker "${MARKER}" must be present`)
assert.match(resolved, new RegExp(MARKER.replace(/[()]/g, "\\$&")),
  "the marker must be emitted when the per-profile root resolves")

// ---------------------------------------------------------------------------
// swap CORE module exposes the per-profile root over its interface, read from
// the host-stamped `instancePersistencePath` property on the module's LogosAPI
// (captured by the generated provider's onInit and stashed in the delivery
// adapter). The LogosModuleContext mixin is deliberately NOT used: it lives in
// a newer cpp-sdk than this module builds against (SDK 0.1.0).
// ---------------------------------------------------------------------------
const adapterCpp = source("swap-module/src/swap_delivery_adapter.cpp")
const adapterHdr = source("swap-module/src/swap_delivery_adapter.h")
assert.match(implHdr, /std::string persistenceRoot\(\);/,
  "SwapImpl must declare persistenceRoot() on its interface")
assert.doesNotMatch(implHdr, /#include\s+"logos_module_context\.h"/,
  "swap_impl.h must not include the LogosModuleContext mixin (absent in SDK 0.1.0)")
assert.doesNotMatch(implHdr, /class\s+SwapImpl\s*:\s*public\s+LogosModuleContext/,
  "SwapImpl must not inherit LogosModuleContext (absent in SDK 0.1.0)")
assert.match(cppBody(implCpp, "SwapImpl::persistenceRoot"), /swapDeliveryRuntimePersistencePath\(\)/,
  "persistenceRoot() must read the runtime-provided path via the delivery adapter")
assert.match(adapterHdr, /std::string swapDeliveryRuntimePersistencePath\(\);/,
  "the delivery adapter must declare the persistence-path accessor")
assert.match(adapterCpp, /property\("instancePersistencePath"\)/,
  "the accessor must read the host-stamped instancePersistencePath property")

console.log("swap_ui per-profile persistence paths: OK")
