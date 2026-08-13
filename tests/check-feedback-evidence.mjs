import assert from "node:assert/strict"
import { readdirSync, readFileSync, statSync } from "node:fs"

function source(path) {
  return readFileSync(new URL(`../${path}`, import.meta.url), "utf8")
}

function functionBody(text, name) {
  const start = text.indexOf(`function ${name}(`)
  assert.notEqual(start, -1, `missing ${name}()`)
  const open = text.indexOf("{", start)
  let depth = 0
  let quote = ""
  let escaped = false
  for (let i = open; i < text.length; i++) {
    const char = text[i]
    if (quote) {
      if (escaped) escaped = false
      else if (char === "\\") escaped = true
      else if (char === quote) quote = ""
      continue
    }
    if (char === '"' || char === "'") {
      quote = char
      continue
    }
    if (char === "{") depth += 1
    if (char === "}" && --depth === 0) return text.slice(open + 1, i)
  }
  assert.fail(`unterminated ${name}()`)
}

const receipt = source("swap-ui/src/qml/ReceiptCard.qml")
const hexValue = source("swap-ui/src/qml/HexValue.qml")
const taker = source("swap-ui/src/qml/TakerView.qml")
const evidence = functionBody(receipt, "safeEvidenceJson")

const allowedKeys = new Set([
  "schema", "role", "status", "ethereum", "chain_id", "lock_tx",
  "lock_url", "claim_tx", "claim_url", "refund_tx", "refund_url", "lez",
  "started_ms", "finished_ms"
])
const actualKeys = new Set(
  [...evidence.matchAll(/^\s{12,}([a-z_]+):/gm)].map((match) => match[1])
)
assert.deepEqual([...actualKeys].sort(), [...allowedKeys].sort(),
  "the public evidence object keys must match the reviewed allowlist exactly")
assert.match(evidence, /r\.iteration\s*=\s*iterationValue/,
  "the optional non-secret loop iteration may be appended")

for (const forbidden of [
  "preimage", "hashlock", "ethRpc", "ethHtlcAddress", "lezProgramId",
  "counterpartyEth", "counterpartyLez", "errorMessage", "parsed", "config",
  "ctx", "networkCtx",
  "mnemonic", "privateKey", "password", "ethPrivateKey", "lezSigningKey",
  "resultJson", "card"
]) {
  assert.doesNotMatch(evidence, new RegExp(`\\b${forbidden}\\b`),
    `${forbidden} crossed the public evidence allowlist`)
}

for (const required of [
  'schema: "trial-evidence/1"', "role: role", "status: outcome",
  "chain_id:", "lock_tx:", "lock_url:", "claim_tx:", "claim_url:",
  "refund_tx:", "refund_url:", "started_ms:", "finished_ms:"
]) {
  assert.ok(evidence.includes(required), `missing safe evidence field: ${required}`)
}

// Execute the actual extracted QML JavaScript with secret sentinels in every
// nearby input. Exact deep equality catches quoted keys, `r.key = ...`
// mutations, and secret values smuggled through an otherwise allowed key.
const argumentNames = [
  "role", "outcome", "ethChainId", "ethLockTx", "ethClaimTx", "ethRefundTx",
  "lezLockTx", "lezClaimTx", "lezRefundTx", "lezExplorerAvailable",
  "startedMs", "finishedMs", "iterationValue", "Links", "preimage", "hashlock",
  "ethRpc", "ethHtlcAddress", "lezProgramId", "counterpartyEth",
  "counterpartyLez", "errorMessage", "parsed", "config", "ctx", "mnemonic",
  "privateKey", "password", "ethPrivateKey", "lezSigningKey", "resultJson",
  "card", "networkCtx"
]
const evaluate = new Function(...argumentNames, `
  "use strict"
  function safeEvidenceJson() {${evidence}}
  return JSON.parse(safeEvidenceJson())
`)
const secret = "SECRET_SENTINEL_MUST_NEVER_APPEAR"
const values = {
  ethChainId: 11155111,
  ethLockTx: "0xeth-lock",
  ethClaimTx: "0xeth-claim",
  ethRefundTx: "0xeth-refund",
  lezLockTx: "lez-lock",
  lezClaimTx: "lez-claim",
  lezRefundTx: "lez-refund",
  lezExplorerAvailable: true,
  startedMs: 100,
  finishedMs: 200,
  Links: {
    ethTx: (hash) => hash ? `https://sepolia.etherscan.io/tx/${hash}` : "",
    lezTx: (hash) => hash
      ? `https://explorer.testnet.lez.logos.co/transaction/${hash}` : ""
  },
  preimage: secret,
  hashlock: secret,
  ethRpc: secret,
  ethHtlcAddress: secret,
  lezProgramId: secret,
  counterpartyEth: secret,
  counterpartyLez: secret,
  errorMessage: secret,
  parsed: { error: secret, preimage: secret },
  config: { privateKey: secret, password: secret },
  ctx: { mnemonic: secret, privateKey: secret, password: secret },
  mnemonic: secret,
  privateKey: secret,
  password: secret,
  ethPrivateKey: secret,
  lezSigningKey: secret,
  resultJson: secret,
  card: { resultJson: secret, preimage: secret },
  networkCtx: { ethRpc: secret, lezSequencer: secret }
}
function evaluated({ role, outcome, iterationValue, lezExplorerAvailable }) {
  const scenario = { role, outcome, iterationValue, lezExplorerAvailable }
  const args = argumentNames.map((name) => name in scenario
    ? scenario[name] : values[name])
  return evaluate(...args)
}
function expected({ role, outcome, iterationValue, lezExplorerAvailable }) {
  const result = {
    schema: "trial-evidence/1",
    role,
    status: outcome,
    ethereum: {
      chain_id: 11155111,
      lock_tx: "0xeth-lock",
      lock_url: "https://sepolia.etherscan.io/tx/0xeth-lock",
      claim_tx: "0xeth-claim",
      claim_url: "https://sepolia.etherscan.io/tx/0xeth-claim",
      refund_tx: "0xeth-refund",
      refund_url: "https://sepolia.etherscan.io/tx/0xeth-refund"
    },
    lez: {
      lock_tx: "lez-lock",
      lock_url: lezExplorerAvailable
        ? "https://explorer.testnet.lez.logos.co/transaction/lez-lock" : null,
      claim_tx: "lez-claim",
      claim_url: lezExplorerAvailable
        ? "https://explorer.testnet.lez.logos.co/transaction/lez-claim" : null,
      refund_tx: "lez-refund",
      refund_url: lezExplorerAvailable
        ? "https://explorer.testnet.lez.logos.co/transaction/lez-refund" : null
    },
    started_ms: 100,
    finished_ms: 200
  }
  if (iterationValue !== null) result.iteration = iterationValue
  return result
}

// Exercise every role/outcome branch the receipt card can render. This makes
// conditional additions fail closed too: a maker-only, error-only, refund-only,
// explorer-only, or iteration-only field must still match the exact contract.
for (const role of ["taker", "maker"]) {
  for (const outcome of ["completed", "error", "failed", "refunded"]) {
    for (const lezExplorerAvailable of [false, true]) {
      for (const iterationValue of [null, 7]) {
        const scenario = { role, outcome, iterationValue, lezExplorerAvailable }
        const actual = evaluated(scenario)
        assert.deepEqual(actual, expected(scenario),
          `runtime evidence must match the public contract: ${JSON.stringify(scenario)}`)
        assert.doesNotMatch(JSON.stringify(actual), new RegExp(secret),
          `a secret sentinel crossed the runtime boundary: ${JSON.stringify(scenario)}`)
      }
    }
  }
}

function filesBelow(path) {
  const absolute = new URL(`../${path}`, import.meta.url)
  const result = []
  for (const entry of readdirSync(absolute)) {
    const child = new URL(`${entry}`, absolute.href.endsWith("/") ? absolute : `${absolute}/`)
    if (statSync(child).isDirectory()) result.push(...filesBelow(`${path}/${entry}`))
    else result.push(`${path}/${entry}`)
  }
  return result
}

for (const qml of filesBelow("swap-ui/src/qml").filter((path) => path.endsWith(".qml"))) {
  assert.doesNotMatch(source(qml), /Qt\.openUrlExternally/,
    `${qml}: Basecamp modules must not attempt direct external navigation`)
}
// The trust-row idiom used to live in TrustRow.qml; HexValue replaced it
// across the receipt, the result card and the offer board's detail pane, so
// the same guarantees are asserted against HexValue now.
//
// Basecamp blocks module-owned external navigation, so COPYING the explorer
// URL is the real action rather than opening it — and it has to be reachable
// without a mouse.
assert.match(hexValue, /component GlyphButton: Button/,
  "trust-row copy/link controls must be Buttons, not bare MouseAreas")
assert.match(hexValue, /Accessible\.onPressAction:\s*glyph\.clicked\(\)/,
  "trust-row controls must advertise a press action to assistive tech")
assert.match(hexValue, /onClicked:\s*row\.copyText\(row\.link, "link"\)/,
  "explorer URLs need an explicit copy action")
assert.match(hexValue, /onClicked:\s*row\.copyText\(row\.value, "value"\)/,
  "values need an explicit copy action")
// One MouseArea is permitted, and only for the show-full-value toggle. A
// second one would almost certainly be a copy action regressing to a
// mouse-only affordance — which is what the TrustRow rule existed to prevent.
assert.equal((hexValue.match(/MouseArea\s*\{/g) || []).length, 1,
  "HexValue must have exactly one MouseArea (the show-full-value toggle)")
assert.match(hexValue,
  /MouseArea\s*\{[\s\S]*?onClicked:\s*row\.expanded = !row\.expanded/,
  "the only MouseArea must be the value expand toggle, never a copy action")
assert.doesNotMatch(receipt, /function receiptJson\(/,
  "the UI must not offer the raw secret-bearing receipt for public feedback")
assert.match(receipt, /feedbackUrl:\s*"https:\/\/github\.com\/logos-co\/eth-lez-atomic-swaps\/issues\/new\?template=trial-feedback\.yml"/,
  "the feedback CTA must use the reviewed HTTPS issue form")
const latestReceiptBody = functionBody(taker, "latestReceiptForRole")
const latestReceiptForRole = new Function("receiptsJson", "wantedRole", `
  "use strict"
  ${latestReceiptBody}
`)
const newestTaker = { role: "taker", id: "newest-taker" }
const newestMaker = { role: "maker", id: "newest-maker" }
const olderTaker = { role: "taker", id: "older-taker" }
const receiptFixture = JSON.stringify([newestMaker, newestTaker, olderTaker])
assert.deepEqual(latestReceiptForRole(receiptFixture, "taker"), newestTaker,
  "the immediate card must select the newest taker receipt")
assert.deepEqual(latestReceiptForRole(receiptFixture, "maker"), newestMaker,
  "receipt selection must remain role-specific")
assert.equal(latestReceiptForRole("malformed", "taker"), null,
  "malformed history must not hide the live result card")

const publicContextBody = functionBody(taker, "publicReceiptContext")
const publicReceiptContext = new Function("receipt", `
  "use strict"
  ${publicContextBody}
`)
assert.deepEqual(publicReceiptContext({
  eth: { lock_tx: "0xlock", counterparty: secret },
  network: { lez_sequencer: "https://lez.example", eth_chain_id: 11155111,
    eth_rpc: secret },
  preimage: secret
}), {
  ethLockTx: "0xlock",
  network: { lezSequencer: "https://lez.example", ethChainId: 11155111 }
}, "the immediate card must receive only its reviewed public receipt context")
assert.deepEqual(publicReceiptContext(null), {
  ethLockTx: "",
  network: { lezSequencer: "", ethChainId: 0 }
}, "missing history must produce an empty public receipt context")

console.log("public feedback evidence boundary: OK")
