// Build the static offer board into dist/: bundle app.js, copy static files.
import { build } from "esbuild";
import { mkdirSync, copyFileSync } from "node:fs";

mkdirSync("dist", { recursive: true });

await build({
  entryPoints: ["app.js"],
  bundle: true,
  minify: true,
  format: "iife",
  outfile: "dist/app.bundle.js",
  logLevel: "info",
  define: { "process.env.NODE_ENV": '"production"' },
});

for (const file of ["index.html", "config.js"]) {
  copyFileSync(file, `dist/${file}`);
}
console.log("built dist/");
