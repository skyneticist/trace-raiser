# pcb-core

Rust geometry engine for converting a routed KiCad 6+ board into a printable,
raised-trace binary STL. The native Rust API is `parse_kicad(&str) -> Board`
and `generate_stl(&Board, &Settings) -> Vec<u8>`.

## Raw WebAssembly ABI

The `wasm32-unknown-unknown` build has no JavaScript glue dependency and exports:

- `alloc(len: u32) -> u32`
- `dealloc(ptr: u32, len: u32)`
- `parse_kicad(ptr: u32, len: u32) -> u64`
- `generate_stl(board_ptr: u32, board_len: u32, settings_ptr: u32, settings_len: u32) -> u64`
- `last_error() -> u64`

Inputs are UTF-8 bytes copied into WASM memory returned by `alloc`. A nonzero
`u64` result packs an owned output buffer as `(pointer << 32) | length`. In
JavaScript, treat the result as a `BigInt`; the pointer is `Number(result >>
32n)` and length is `Number(result & 0xffffffffn)`. Copy the output before
calling `dealloc(pointer, length)`.

`parse_kicad` always returns UTF-8 JSON. On success it is a flat `Board` object;
on failure it is `{ "error": "message" }`. `generate_stl` returns a binary STL,
or zero on failure; `last_error` then returns the UTF-8 error message and clears
it.

## JSON contracts

```text
Board {
  name,
  bounds: { min_x, min_y, max_x, max_y, width, height },
  outline: [{ x, y }],
  traces: [{ start:{x,y}, end:{x,y}, width, layer, net_id, net_name }],
  pads: [{ position:{x,y}, size:{x,y}, drill:null|number,
           pad_type, shape, rotation, layers:[string], net_id, net_name }],
  vias: [{ position:{x,y}, size, drill, layers:[string], net_id, net_name }],
  zones: [{ layer, net_id, net_name, name,
            kind:"copper"|"teardrop"|"keepout",
            polygons:[[{x,y}]] }],
  warnings: [{ code, message, severity:"info"|"warning"|"error" }],
  stats: { traces, pads, vias, holes }
}

Settings {
  board_thickness, trace_height, trace_width,
  hole_compensation, side,
  width_mode: "auto" | "preserve",
  trace_style: "technical" | "soft" | "vintage",
  neckdown_width, taper_length,
  corner_radius, teardrop_length, teardrop_strength,
  trace_clearance
}
```

`trace_width` is a floor applied to every routed trace; a wider source trace is
not narrowed. `side` remains accepted for compatibility but V1 intentionally
always emits mirrored `B.Cu` for a single-sided printable board. It may be
omitted and defaults to `B.Cu`. All STL coordinates are translated so the board
bounds start at `(0, 0)`.

In `auto` mode, `trace_width` is the wide-trunk floor. A trace endpoint attached
to a same-net through-hole pad remains at `neckdown_width` through the pad
boundary, then tapers to the trunk over `taper_length`. `technical` retains the
original linear envelope, `soft` uses quintic zero-slope easing, and `vintage`
adds explicit monotonic tangent Bézier pad lobes plus constant-radius circular
fillets at eligible degree-2 same-circuit nodes. Pad/via nodes, branches,
ambiguous nets, and corners too short for a safe offset remain unmodified.
Two-ended short segments combine both endpoint constraints without exceeding
either envelope and remain valid even when they cannot reach full trunk width.
`preserve` mode emits the raw KiCad segment width uniformly without tapering.
Old JSON without these fields receives safe compatibility defaults: `auto`,
`technical`, 1.4 mm neck, 4.0 mm taper, 3.0 mm corner radius, 3.0 mm teardrop
length, 0.75 teardrop strength, and 0.5 mm clearance.

Known nets are unioned independently before export. Same-net overlaps and
T-junctions are legal; different-net intersection or less than
`trace_clearance` returns a precise generation error with both net labels and a
representative board coordinate. Missing net metadata remains conservative,
while exact legacy trace-endpoint-to-pad-center connections are retained.

The parser supports `gr_rect`, `gr_poly`, and chained `gr_line` Edge.Cuts,
straight F.Cu/B.Cu segments, tessellated routed copper arcs, vias, rotated
footprint pads, and KiCad's cached `B.Cu` `filled_polygon` zone geometry.
Keepouts are metadata only because their effect is already present in cached
fills. An unfilled B.Cu zone is an explicit export-blocking error; Copperline
does not approximate KiCad thermal, priority, island, or clearance rules from
the zone outline. Custom pads, SMD pads, and approximated bottom-footprint
transforms become explicit warnings instead of silently changing connectivity.
`np_thru_hole` pads create base holes but never raised copper; only `thru_hole`
pads create annular raised features. Edge.Cuts inner loops are reported but are
not supported as board cutouts in V1.

## Unified export geometry

All printable B.Cu trace capsules, supported through-hole pad polygons, and via
footprints are boolean-unioned with `i_overlay`, clipped to Edge.Cuts, and have
all compensated drills subtracted before triangulation. The resulting board is
emitted as one stepped boundary mesh rather than overlapping feature shells:

- board/copper partition faces at `z = 0`
- exterior board and drill walls through `board_thickness`
- uncovered board top at `board_thickness`
- raised-copper walls and top at `board_thickness + trace_height`

There are no copper bottom faces or hidden board-top faces under raised copper.
The fixed boolean grid is 0.00001 mm, and transition vertices introduced where
copper meets a board or drill edge are preserved in the lower walls. Generation
rejects consumed annuli, overlapping/out-of-board drills, and point-only copper
tangencies instead of returning a non-manifold STL.
