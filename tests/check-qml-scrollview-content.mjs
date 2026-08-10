#!/usr/bin/env node
// Structural guard: a ScrollView-rooted view must have exactly ONE direct
// child object, and it must be a visual content item (never a non-visual
// helper like Timer or Connections).
//
// Why this exists — the 0.4.2 Setup-tab overlap regression (#113):
// SetupView.qml declared a `Timer` and a `Connections` as the FIRST children
// of its root `ScrollView`, ahead of the `Flickable` that holds the whole
// layout. ScrollView adopts the *first* Flickable in its contentData as its
// scroll surface; because two non-Flickable objects were appended first,
// ScrollView spun up its OWN implicit Flickable and demoted the real one to a
// zero-sized ordinary child. Zero width propagated into the ColumnLayout
// (measured width -64), the layout pass bailed, and every element collapsed
// onto the origin — the "everything overlaps in the top-left" screenshot.
//
// qmllint and the build could not catch it: the QML is well-formed and every
// binding resolves; the damage is a *runtime* content-slot mis-assignment that
// only shows up once ScrollView lays out. Non-visual helpers (Timer,
// Connections, …) belong INSIDE the content Flickable, which is the convention
// every other ScrollView view here already follows. This greppable check turns
// that whole class into a test failure with zero Qt/runtime dependencies.
//
// Run: node tests/check-qml-scrollview-content.mjs

import { readdirSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const qmlDir = join(dirname(fileURLToPath(import.meta.url)), "..", "swap-ui", "src", "qml");

// Object types that are QtObject-derived (non-visual) and therefore must never
// be a *direct* child of a ScrollView — they steal the single-visual-child
// content slot. Nest them inside the content item instead.
const NON_VISUAL = new Set([
  "Timer", "Connections", "QtObject", "Instantiator", "FontLoader",
  "SystemPalette", "Settings", "Component", "Timeline", "TextMetrics",
]);

// Walk `src` and return the list of { type, line } for every object
// declaration that is a DIRECT child of the first (root) object — i.e. at brace
// depth 1. Comments and string literals are neutralised so their braces and
// keywords never register. A "child object" is `Identifier {` where Identifier
// is capitalised and is NOT preceded by `:` (which would make it a grouped /
// assigned property value such as `background: Rectangle { … }`).
function rootDirectChildren(src) {
  let depth = 0;
  let i = 0;
  const n = src.length;
  let line = 1;
  let prevSig = "";          // last significant (non-ws, non-comment) char
  const children = [];

  // Current identifier being accumulated, and the last *completed* token —
  // its start line and the significant char immediately before it. A token
  // opens an object only when the very next significant char is `{` with
  // nothing but whitespace in between (`onlySpaceSinceToken`).
  let cur = "";
  let startPrev = "";
  let startLine = 0;
  let lastToken = "";
  let lastTokenPrev = "";
  let lastTokenLine = 0;
  let onlySpaceSinceToken = false;

  const complete = () => {
    if (cur !== "") {
      lastToken = cur;
      lastTokenPrev = startPrev;
      lastTokenLine = startLine;
      cur = "";
      return true;
    }
    return false;
  };

  while (i < n) {
    const c = src[i];
    // Line comment
    if (c === "/" && src[i + 1] === "/") {
      complete();
      while (i < n && src[i] !== "\n") i++;
      continue;
    }
    // Block comment
    if (c === "/" && src[i + 1] === "*") {
      complete();
      i += 2;
      while (i < n && !(src[i] === "*" && src[i + 1] === "/")) {
        if (src[i] === "\n") line++;
        i++;
      }
      i += 2;
      continue;
    }
    // String literals — a significant token that is never a type name.
    if (c === '"' || c === "'") {
      complete();
      const q = c;
      i++;
      while (i < n && src[i] !== q) {
        if (src[i] === "\\") i++;
        if (src[i] === "\n") line++;
        i++;
      }
      i++;
      prevSig = q;
      onlySpaceSinceToken = false;
      continue;
    }
    // Identifier char
    if (/[A-Za-z0-9_]/.test(c)) {
      if (cur === "") { startPrev = prevSig; startLine = line; }
      cur += c;
      prevSig = c;
      i++;
      continue;
    }
    // Whitespace — closes the current token but keeps the "only space" chain.
    if (c === " " || c === "\t" || c === "\r" || c === "\n") {
      if (c === "\n") line++;
      if (complete()) onlySpaceSinceToken = true;
      i++;
      continue;
    }
    // Significant punctuation.
    if (c === "{") {
      if (complete()) onlySpaceSinceToken = true;
      if (
        depth === 1 &&
        onlySpaceSinceToken &&
        /^[A-Z]/.test(lastToken) &&
        lastTokenPrev !== ":"
      ) {
        children.push({ type: lastToken, line: lastTokenLine });
      }
      depth++;
      lastToken = "";
      onlySpaceSinceToken = false;
      prevSig = c;
      i++;
      continue;
    }
    if (c === "}") {
      complete();
      depth--;
      lastToken = "";
      onlySpaceSinceToken = false;
      prevSig = c;
      i++;
      continue;
    }
    // Any other punctuation (`:`, `;`, `(`, `)`, `.`, `,`, operators…) breaks
    // the token-adjacent-to-brace chain.
    complete();
    onlySpaceSinceToken = false;
    prevSig = c;
    i++;
  }
  return children;
}

function rootType(src) {
  // First capitalised `Identifier {` at depth 0.
  const noComments = src
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
  const m = noComments.match(/(^|\n)\s*([A-Z]\w*)\s*\{/);
  return m ? m[2] : null;
}

const files = readdirSync(qmlDir).filter((f) => f.endsWith(".qml"));
const failures = [];
let scrollViews = 0;

for (const file of files) {
  const src = readFileSync(join(qmlDir, file), "utf8");
  if (rootType(src) !== "ScrollView") continue;
  scrollViews += 1;
  const children = rootDirectChildren(src);

  const helpers = children.filter((c) => NON_VISUAL.has(c.type));
  for (const h of helpers) {
    failures.push(
      `${file}:${h.line}: ${h.type} is a direct child of the root ScrollView. ` +
      `Non-visual helpers steal ScrollView's single content slot (its Flickable) ` +
      `and collapse the layout to the origin — move it INSIDE the content Flickable.`
    );
  }
  if (children.length > 1) {
    const list = children.map((c) => `${c.type}@${c.line}`).join(", ");
    failures.push(
      `${file}: root ScrollView has ${children.length} direct child objects (${list}); ` +
      `it supports exactly one visual child. Wrap the extras inside the content item.`
    );
  }
  if (children.length === 0) {
    failures.push(`${file}: root ScrollView has no content child object`);
  }
}

if (scrollViews === 0) {
  console.error("FAIL: no ScrollView-rooted views found — the checker is miswired");
  process.exit(1);
}
if (failures.length > 0) {
  console.error("FAIL: ScrollView content-slot violations:");
  for (const f of failures) console.error("  " + f);
  process.exit(1);
}

console.log(`qml scrollview content contract: OK (${scrollViews} ScrollView-rooted views, single visual child each)`);
