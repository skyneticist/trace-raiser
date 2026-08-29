# kicad-layout

`kicad-layout` is the loss-minimizing KiCad boundary for Copperline AutoLayout.
It parses a bounded KiCad PCB S-expression into first-class footprint, pad, net,
courtyard, and Edge.Cuts records while retaining byte spans into the original
document. Its design bridge turns supported boards into the canonical
`layout-core` model without guessing component bodies from pad extents.

Accepted placement changes are written by replacing only the immediate `(at …)`
expression of the selected top-level footprint. Unknown tokens, comments,
formatting, UUIDs, graphics, rules, zones, and routed copper remain byte-for-byte
unchanged. A no-op rewrite returns the original input exactly.

This crate deliberately does not claim to be a complete KiCad serializer. It is
a narrow, fail-closed adapter for placement experiments. KiCad remains the final
parser and DRC authority for every generated candidate.

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

From the repository root, run the complete contract suite with:

```sh
REQUIRE_KICAD_DRC=1 \
KICAD_CLI=/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli \
scripts/check-autolayout.sh
```

This crate is currently an engine boundary, not a finished end-user command.
The placement/router pipeline and preview/accept UI will call it after corpus
evaluation and routing are implemented.
