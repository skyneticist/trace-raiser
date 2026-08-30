# layout-core

layout-core is Copperline AutoLayout's deterministic optimization boundary. It
contains no KiCad parser and no browser state. A versioned JSON design goes in;
replayable placement and routing candidates come out.

Current implementation:

- validates IDs, nets, terminals, simple outlines, THT/NPTH pads, locked poses,
  placement envelopes, keepouts, clearances, rotations, and jumper authority;
- places front-side THT components on a bounded grid using deterministic
  global hints, exact legalization, and coordinate refinement;
- routes B.Cu with bounded deterministic A*, multi-terminal tree growth, and
  history-guided rip-up/reroute;
- reports complete, infeasible_fixed_constraints, or search_exhausted without
  turning a partial result into a false success;
- independently validates route connectivity, edge containment, trace-to-trace
  and trace-to-pad clearance, metrics, and exact user-approved jumpers;
- produces review-only jumper proposals and mints SHA-256 content-bound approval
  IDs only through an explicit acceptance operation;
- serializes every input and result with an explicit schema version.

Run the engine against the checked-in fixture:

~~~bash
cargo run --manifest-path crates/layout-core/Cargo.toml \
  --example place_json -- \
  crates/layout-core/fixtures/two-pin.json -

cargo run --manifest-path crates/layout-core/Cargo.toml \
  --example route_json -- \
  crates/layout-core/fixtures/routing-smoke.json -
~~~

Exit codes are 0 for a complete placement, 2 for infeasible fixed constraints,
3 for an exhausted search, and 1 for invalid input or I/O.

The optimizers remain replaceable behind the versioned model, independent
validators, fixtures, metrics, deterministic seeds, and explicit failure
states.
