# Copperline Studio

Copperline Studio turns a routed KiCad board into raised-trace geometry for the
FDM-and-copper-tape prototyping method. Parsing and mesh generation run locally
in a Rust/WebAssembly core; the browser interface previews the reconstructed
board and exports slicer-ready 3MF or STL files.

## Current MVP

- Imports routed KiCad 6+ `.kicad_pcb` files.
- Reconstructs straight and curved back-copper tracks, standard drilled through-hole pads,
  NPTH mounting holes, vias, and simple rectangular, polygonal, or line-chain
  board outlines.
- Produces mirrored `B.Cu` geometry for a single-sided board and blocks
  unsupported SMD, connector, or custom copper pads.
- Automatically keeps configurable narrow neck-downs through through-hole pads,
  then tapers them into wider printable trunks after clearing the pad boundary.
- Offers Technical, Soft, and Vintage trace styles. Soft uses zero-slope eased
  tapers; Vintage adds monotonic tangent Bézier lobes at pads and tangent bends
  at safe, unbranched same-net corners.
- Preserves KiCad's cached filled `B.Cu` zone polygons exactly, including
  thermal reliefs, islands, and keepout-shaped voids. Unfilled back-copper zones
  block export with a request to refill and save them in KiCad.
- Can instead preserve every KiCad segment width exactly, and adjusts substrate
  thickness, raised-trace height, clearance, and hole compensation.
- Keeps compensated drills open through both the substrate and every raised
  feature, including traces that terminate at a pad center.
- Unions connected and overlapping copper into one watertight stepped mesh,
  without hidden faces or internal walls between KiCad trace segments.
- Retains KiCad net identity, permits same-net joins, and marks detected
  different-net trace, pad, via, and filled-zone clearance conflicts in the
  preview.
- Exports 3MF, binary STL, and a reopenable `.copperline.json` project.
- Stores a device-local preferred slicer and offers a separate 3MF handoff;
  browsers without native file sharing fall back to a normal download.
- Keeps board files and generated geometry in the browser.

Routed copper arcs are deterministically tessellated at a small chord error.
Copperline consumes KiCad's saved zone fill rather than implementing a partial
zone-filling engine; press **B** to refill zones in KiCad before saving and
importing. The tool is intended for simple, single-sided, through-hole,
low-voltage hobby prototypes—not mains, RF, high-current, safety-critical, or
production boards. Always inspect the generated model in a slicer and verify
continuity and isolation with a meter before applying power.

## Development

```bash
npm install
npm run wasm:build
npm run dev
```

Quality checks:

```bash
npm test
npm run test:core
npm run lint
```

`npm test` rebuilds `public/pcb_core.wasm` before the production build and
browser/WASM regression suite so the shipped engine cannot silently lag behind
the Rust source.

The Rust ABI and geometry contract are documented in
[`crates/pcb-core/README.md`](crates/pcb-core/README.md).

## Why a routed board is required

A schematic contains connectivity, but it does not contain physical placement,
footprint, outline, or routing geometry. Copperline's separate
[AutoLayout foundation](docs/autolayout/FOUNDATION.md) now accepts a KiCad PCB
with footprints and an outline, and provides deterministic placement and
single-layer routing. Footprint assignment, syntax-preserving KiCad track
emission, and integration into the Studio upload flow are still later product
stages.
