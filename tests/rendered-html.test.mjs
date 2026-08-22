import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import test from "node:test";

async function render() {
  const workerUrl = new URL("../dist/server/index.js", import.meta.url);
  workerUrl.searchParams.set("test", `${process.pid}-${Date.now()}`);
  const { default: worker } = await import(workerUrl.href);

  return worker.fetch(
    new Request("http://localhost/", { headers: { accept: "text/html" } }),
    {
      ASSETS: { fetch: async () => new Response("Not found", { status: 404 }) },
      IMAGES: {},
    },
    { waitUntil() {}, passThroughOnException() {} },
  );
}

test("server-renders the Copperline workspace", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html\b/i);

  const html = await response.text();
  assert.match(html, /<title>Copperline Studio — KiCad to printable PCB form<\/title>/i);
  assert.match(html, /Workspace tools/);
  assert.match(html, />Source</);
  assert.match(html, />Shape</);
  assert.match(html, />Check</);
  assert.match(html, />Export</);
  assert.match(html, /ACTIVE BOARD/);
  assert.match(html, /Generated board preview/);
  assert.match(html, />RELIEF</);
  assert.match(html, />TOP</);
  assert.match(html, /Open board checks/);
  assert.match(html, /Rust · WASM/);
  assert.doesNotMatch(html, /codex-preview|Your site is taking shape/);
});

test("keeps source, shaping, diagnostics, and export controls behind the workspace drawers", async () => {
  const [workspaceSource, slicerSource] = await Promise.all([
    readFile(new URL("../app/pcb-studio.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/lib/slicer-handoff.ts", import.meta.url), "utf8"),
  ]);
  const source = `${workspaceSource}\n${slicerSource}`;

  for (const label of [
    "Choose board",
    "Back copper only",
    "Trace height",
    "Auto neck-down",
    "Preserve KiCad",
    "Trunk width",
    "Neck-down width",
    "Taper length",
    "Trace style",
    "Vintage shaping",
    "Clearance",
    "Download 3MF",
    "Preferred slicer",
    "Bambu Studio",
    "OrcaSlicer",
    "PrusaSlicer",
    "Ultimaker Cura",
    "System chooser",
    "Save editable project",
  ]) {
    assert.match(source, new RegExp(label));
  }
});

test("ships the local geometry engine and production metadata", async () => {
  const [wasm, social, packageJson, page, layout, sample] = await Promise.all([
    stat(new URL("../public/pcb_core.wasm", import.meta.url)),
    stat(new URL("../public/og-v2.png", import.meta.url)),
    readFile(new URL("../package.json", import.meta.url), "utf8"),
    readFile(new URL("../app/page.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/layout.tsx", import.meta.url), "utf8"),
    readFile(new URL("../public/sample-sensor.kicad_pcb", import.meta.url), "utf8"),
  ]);

  assert.ok(wasm.size > 100_000, "expected a compiled Rust/WASM engine");
  assert.ok(social.size > 100_000, "expected a bespoke social preview card");
  assert.match(packageJson, /"name": "copperline-studio"/);
  assert.match(packageJson, /"fflate"/);
  assert.doesNotMatch(packageJson, /react-loading-skeleton/);
  assert.match(page, /CopperlineStudio/);
  assert.match(layout, /generateMetadata/);
  assert.match(layout, /\/og-v2\.png/);
  assert.match(sample, /^\(kicad_pcb/);
  assert.match(sample, /\(layer "B\.Cu"\)/);
  assert.match(sample, /thru_hole/);
  assert.doesNotMatch(sample, /\spad\s+"[^"]+"\s+smd\s/);
});

test("compiled WebAssembly exposes the browser loader ABI", async () => {
  const bytes = await readFile(new URL("../public/pcb_core.wasm", import.meta.url));
  const wasmModule = await WebAssembly.compile(bytes);
  const instance = await WebAssembly.instantiate(wasmModule, {});

  assert.equal(typeof instance.exports.alloc, "function");
  assert.equal(typeof instance.exports.dealloc, "function");
  assert.equal(typeof instance.exports.parse_kicad, "function");
  assert.equal(typeof instance.exports.generate_stl, "function");
  assert.ok(instance.exports.memory instanceof WebAssembly.Memory);
});
