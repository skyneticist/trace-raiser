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
  assert.match(html, /Source board/);
  assert.match(html, /Build setup/);
  assert.match(html, /Board report/);
  assert.match(html, /Export model/);
  assert.match(html, /ACTIVE BOARD/);
  assert.match(html, /Generated board preview/);
  assert.match(html, />RELIEF</);
  assert.match(html, />TOP</);
  assert.match(html, /Switch to dim theme/);
  assert.match(html, /Vintage profile/);
  assert.match(html, /Rust · WASM/);
  assert.doesNotMatch(html, /workspace-drawer|workflow-rail|drawer-backdrop|drawer-close|role="tab"|<details/i);
  assert.doesNotMatch(html, /codex-preview|Your site is taking shape/);
});

test("renders source, shaping, diagnostics, and export together on one page", async () => {
  const [response, workspaceSource, globalStyles] = await Promise.all([
    render(),
    readFile(new URL("../app/pcb-studio.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/globals.css", import.meta.url), "utf8"),
  ]);
  const html = await response.text();

  for (const label of [
    "Choose board",
    "Output layer",
    "B.Cu · mirrored · local",
    "Form",
    "Routing",
    "Load 10-part AutoLayout assessment",
    "Create routing proposal",
    "Routing constraints",
    "Trace height",
    "Auto neck-down",
    "Preserve KiCad",
    "Trunk width",
    "Neck-down width",
    "Taper length",
    "Trace style",
    "Vintage profile",
    "Clearance",
    "OUTPUT RECIPE",
    "Overall size",
    "Vertical stack",
    "Profile",
    "Route widths",
    "Download 3MF",
    "Preferred slicer",
    "Bambu Studio",
    "OrcaSlicer",
    "PrusaSlicer",
    "Ultimaker Cura",
    "System chooser",
    "Save editable project",
    "Load a board to enable model export.",
  ]) {
    assert.match(html, new RegExp(label));
  }
  for (const label of [
    "Accept + download",
    "Download candidate again",
    "KiCad remains the authority",
  ]) {
    assert.ok(workspaceSource.includes(label), `expected workspace source to include ${label}`);
  }
  assert.doesNotMatch(html, /workspace-drawer|workflow-rail|drawer-backdrop|drawer-close|<details/i);
  assert.match(workspaceSource, /THEME_PREFERENCE_KEY = "copperline-theme"/);
  assert.match(workspaceSource, /localStorage\.setItem\(THEME_PREFERENCE_KEY, next\)/);
  assert.match(workspaceSource, /model-export-readiness/);
  assert.match(workspaceSource, /validation-summary[\s\S]*displayedStatus/);
  assert.match(workspaceSource, /Raised form preview with[\s\S]*vertical emphasis/);
  assert.match(workspaceSource, /fillExtrudedLayers[\s\S]*drawReliefLegend/);
  assert.match(workspaceSource, /BLOCKER/);
  assert.doesNotMatch(workspaceSource, /header-readiness/);
  assert.match(globalStyles, /@font-face[\s\S]*FiraCode-VF\.woff2/);
  assert.match(globalStyles, /font-variant-ligatures: contextual common-ligatures/);
  assert.match(globalStyles, /font-feature-settings: "calt" 1, "liga" 1/);
  assert.match(globalStyles, /data-theme="dim"/);
  assert.match(globalStyles, /\.controls-pane, \.output-pane[\s\S]*border-radius: 18px/);
  assert.match(globalStyles, /\.preview-panel[\s\S]*margin: 12px 0[\s\S]*border-radius: 18px/);
  assert.match(globalStyles, /backdrop-filter: saturate\(135%\) blur\(18px\)/);
  assert.match(globalStyles, /--fs-meta: 11px/);
});

test("ships the local geometry engine, typeface, and production metadata", async () => {
  const [wasm, autoLayoutWasm, social, font, fontLicense, packageJson, page, layout, sample, unroutedSample, assessmentSample] = await Promise.all([
    stat(new URL("../public/pcb_core.wasm", import.meta.url)),
    stat(new URL("../public/autolayout_core.wasm", import.meta.url)),
    stat(new URL("../public/og-v2.png", import.meta.url)),
    stat(new URL("../public/fonts/FiraCode-VF.woff2", import.meta.url)),
    readFile(new URL("../public/fonts/FiraCode-LICENSE.txt", import.meta.url), "utf8"),
    readFile(new URL("../package.json", import.meta.url), "utf8"),
    readFile(new URL("../app/page.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/layout.tsx", import.meta.url), "utf8"),
    readFile(new URL("../public/sample-sensor.kicad_pcb", import.meta.url), "utf8"),
    readFile(new URL("../public/sample-unrouted.kicad_pcb", import.meta.url), "utf8"),
    readFile(new URL("../public/sample-autolayout-assessment.kicad_pcb", import.meta.url), "utf8"),
  ]);

  assert.ok(wasm.size > 100_000, "expected a compiled Rust/WASM engine");
  assert.ok(autoLayoutWasm.size > 100_000, "expected a compiled AutoLayout WASM engine");
  assert.ok(social.size > 100_000, "expected a bespoke social preview card");
  assert.ok(font.size > 100_000, "expected the self-hosted Fira Code variable font");
  assert.match(fontLicense, /SIL OPEN FONT LICENSE Version 1\.1/i);
  assert.match(packageJson, /"name": "copperline-studio"/);
  assert.match(packageJson, /"fflate"/);
  assert.doesNotMatch(packageJson, /react-loading-skeleton/);
  assert.match(page, /CopperlineStudio/);
  assert.match(layout, /generateMetadata/);
  assert.match(layout, /\/og-v2\.png/);
  assert.doesNotMatch(layout, /next\/font\/google/);
  assert.match(sample, /^\(kicad_pcb/);
  assert.match(sample, /\(layer "B\.Cu"\)/);
  assert.match(sample, /thru_hole/);
  assert.doesNotMatch(sample, /\spad\s+"[^"]+"\s+smd\s/);
  assert.match(unroutedSample, /^\(kicad_pcb/);
  assert.doesNotMatch(unroutedSample, /\(segment\b/);
  assert.match(assessmentSample, /^\(kicad_pcb/);
  assert.equal(assessmentSample.match(/\(footprint /g)?.length, 10);
  assert.equal(assessmentSample.match(/^ {2}\(net (?:[1-9]|1[0-2])\b/gm)?.length, 12);
  assert.equal(assessmentSample.match(/np_thru_hole/g)?.length, 2);
  assert.doesNotMatch(assessmentSample, /\(segment\b/);
});

test("compiled WebAssembly exposes the browser loader ABI", async () => {
  const [bytes, autoLayoutBytes] = await Promise.all([
    readFile(new URL("../public/pcb_core.wasm", import.meta.url)),
    readFile(new URL("../public/autolayout_core.wasm", import.meta.url)),
  ]);
  const [wasmModule, autoLayoutModule] = await Promise.all([
    WebAssembly.compile(bytes),
    WebAssembly.compile(autoLayoutBytes),
  ]);
  const [instance, autoLayoutInstance] = await Promise.all([
    WebAssembly.instantiate(wasmModule, {}),
    WebAssembly.instantiate(autoLayoutModule, {}),
  ]);

  assert.equal(typeof instance.exports.alloc, "function");
  assert.equal(typeof instance.exports.dealloc, "function");
  assert.equal(typeof instance.exports.parse_kicad, "function");
  assert.equal(typeof instance.exports.generate_stl, "function");
  assert.ok(instance.exports.memory instanceof WebAssembly.Memory);
  assert.equal(typeof autoLayoutInstance.exports.alloc, "function");
  assert.equal(typeof autoLayoutInstance.exports.dealloc, "function");
  assert.equal(typeof autoLayoutInstance.exports.auto_layout, "function");
  assert.equal(typeof autoLayoutInstance.exports.last_error, "function");
  assert.ok(autoLayoutInstance.exports.memory instanceof WebAssembly.Memory);
});
