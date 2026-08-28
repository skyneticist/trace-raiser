# kicad-layout

`kicad-layout` is the loss-minimizing KiCad boundary for Copperline AutoLayout.
It parses a bounded KiCad PCB S-expression into first-class footprint, pad, net,
and Edge.Cuts records while retaining byte spans into the original document.

Accepted placement changes are written by replacing only the immediate `(at …)`
expression of the selected top-level footprint. Unknown tokens, comments,
formatting, UUIDs, graphics, rules, zones, and routed copper remain byte-for-byte
unchanged. A no-op rewrite returns the original input exactly.

This crate deliberately does not claim to be a complete KiCad serializer. It is
a narrow, fail-closed adapter for placement experiments. KiCad remains the final
parser and DRC authority for every generated candidate.

Safety boundaries:

- input size, nesting depth, and syntax-node count are limited;
- non-finite placement coordinates are rejected;
- duplicate or missing footprint identities are rejected;
- locked footprints cannot be moved unless the caller explicitly overrides the
  lock;
- unknown placement keys are errors rather than silently ignored requests.
