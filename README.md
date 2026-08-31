# Copperline Studio

Copperline Studio turns a KiCad board into raised-trace geometry for the
FDM-and-copper-tape prototyping method. It can also produce a deterministic,
review-first placement and single-layer routing proposal for a deliberately
narrow class of unrouted boards. Parsing, layout, and mesh generation run
locally in Rust/WebAssembly; the browser previews the reconstructed board and
exports KiCad candidates, 3MF, or STL files.

## Current MVP

- Imports routed KiCad 6+ `.kicad_pcb` files.
- Auto-places front-side through-hole footprints and routes complete proposals
  on `B.Cu` for an unrouted board with supported courtyards and one simple
  outline. Existing copper is never overwritten or merged.
- Keeps an AutoLayout result separate from the source while the user compares
  original and proposed views, metrics, constraints, and a deterministic replay
  ID. Only an explicit **Accept + download** action adopts it in the session.
- Locks printable and project export after acceptance until the downloaded
  candidate has been checked in KiCad and re-imported.
- Reconstructs straight and curved back-copper tracks, standard drilled through-hole pads,
  NPTH mounting holes, vias, and simple rectangular, polygonal, or line-chain
  board outlines.
- Produces mirrored `B.Cu` geometry for a single-sided board and blocks
  unsupported SMD, connector, or custom copper pads.
- Automatically keeps configurable narrow neck-downs through through-hole pads,
  then tapers them into wider printable trunks after clearing the pad boundary.
- Offers Technical, Soft, and Vintage trace styles. Soft uses zero-slope eased
  tapers; Vintage adds monotonic tangent Bézier lobes at pads and constant-radius
  tangent fillets at safe, unbranched same-net corners.
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
- Prepares deterministic 3MF and binary STL artifacts after each geometry
  change so export clicks are immediate, repeatable, and snapshot-safe; also
  saves a reopenable `.copperline.json` project.
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

`npm test` rebuilds both `public/pcb_core.wasm` and
`public/autolayout_core.wasm` before the production build and browser/WASM
regression suite so neither shipped engine can silently lag behind Rust source.

The Rust ABI and geometry contract are documented in
[`crates/pcb-core/README.md`](crates/pcb-core/README.md).

## AutoLayout workflow and boundary

A schematic contains connectivity, but it does not contain the footprint and
board geometry needed for physical layout. Start in KiCad by assigning
footprints, drawing one supported board outline, and leaving the copper
unrouted. Then in Copperline:

1. Open **Source** and load the `.kicad_pcb`, or load the 10-footprint
   assessment board. Its 12 two-terminal nets, three fixed edge connectors,
   fixed series part, and two mounting obstacles provide a more meaningful
   placement-and-routing review than the minimal internal smoke fixture.
2. Choose Balanced or Thorough search and, if needed, adjust the clearances,
   trace width, and deterministic replay seed.
3. Create a proposal and compare **Original** with **Proposed**. The source file
   is still unchanged at this point.
4. Choose **Accept + download** only if the proposal is worth checking.
5. Open the downloaded candidate in KiCad, run the Design Rules Checker, save,
   and re-import that checked board before generating printable output.

If the browser suppresses the first download request, reopen **Source** and use
**Download candidate again**; the accepted candidate remains locked in the
session until a checked board is re-imported.

The current browser cannot prove that KiCad DRC was run; re-import is the
explicit trust boundary. AutoLayout fails closed on partial routing and does not
yet serialize jumpers. SMD, multilayer, RF, impedance-controlled,
safety-critical, and production designs remain outside the supported scope. See
the full [AutoLayout foundation](docs/autolayout/FOUNDATION.md) for the engine
contract and evidence boundary.
