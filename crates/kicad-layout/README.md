# kicad-layout

`kicad-layout` is the loss-minimizing KiCad boundary for Copperline AutoLayout.
It parses a bounded KiCad PCB S-expression into first-class footprint, pad, net,
courtyard, and Edge.Cuts records while retaining byte spans into the original
document. Its design bridge turns supported boards into the canonical
`layout-core` model without guessing component bodies from pad extents.

Accepted placement changes are written by replacing only the immediate `(at …)`
expression of the selected top-level footprint. A complete route can also be
inserted as deterministic `B.Cu` segment nodes immediately before the root
closing parenthesis. Unknown tokens, comments, formatting, UUIDs, graphics, and
rules remain byte-for-byte unchanged. A no-op rewrite returns the original
input exactly, and existing copper is never overwritten or merged.

This crate deliberately does not claim to be a complete KiCad serializer. It is
a narrow, fail-closed adapter for localized placement and single-layer route
candidates. KiCad remains the final parser and DRC authority for every generated
candidate.

Safety boundaries:

- input size, nesting depth, and syntax-node count are limited;
- F.CrtYd is required for front-side footprints unless a trusted manual
  envelope is supplied, and a manual envelope cannot shrink parsed courtyard
  bounds;
- line, rectangle, polygon, circle, and arc geometry is validated; malformed or
  unsupported authoritative geometry fails closed;
- courtyard endpoint chaining matches KiCad's 0.02 mm tolerance, while board
  outline endpoints must coincide on the one-nanometer coordinate grid;
- curved outlines receive a conservative tessellation-clearance guard;
- multiple board loops, holes, branches, and footprint-owned Edge.Cuts are not
  yet supported;
- non-finite placement coordinates are rejected;
- duplicate or missing footprint identities are rejected;
- locked footprints cannot be moved unless the caller explicitly overrides the
  lock;
- unknown placement keys are errors rather than silently ignored requests.
- candidate emission rejects existing copper, incomplete routes, stale design
  geometry or connectivity, and jumper-bearing routes;
- deterministic UUIDv8 track identifiers are derived from candidate content,
  while legacy ordinal and KiCad 10 name-based net syntax are preserved.

From the repository root, run the complete contract suite with:

```sh
REQUIRE_KICAD_DRC=1 \
KICAD_CLI=/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli \
scripts/check-autolayout.sh
```

The intentionally small corpus smoke can also be run directly:

```sh
cargo run --manifest-path crates/kicad-layout/Cargo.toml \
  --example evaluate_corpus -- \
  crates/kicad-layout/fixtures/corpus.json report.json
```

It emits machine-readable placement metrics and checks a stable replay
fingerprint. The checked-in corpus currently contains one representative KiCad
10 success path and one fail-closed geometry path; additional cases are deferred
until a concrete regression or a later test-expansion pass justifies them.

The native validation gate and the dedicated browser AutoLayout WASM module use
this same adapter. Copperline Studio exposes it as a proposal workflow:
original/proposed comparison, explicit accept-and-download, then mandatory
KiCad DRC and re-import before printable export.
