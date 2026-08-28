# layout-core

layout-core is Copperline AutoLayout's deterministic optimization boundary. It
contains no KiCad parser and no browser state. A versioned JSON design goes in;
replayable placement and routing candidates come out.

Current implementation:

- validates IDs, nets, terminals, simple outlines, THT/NPTH pads, locked poses,
  placement envelopes, keepouts, clearances, rotations, and jumper authority;
- places front-side THT components on a bounded grid using deterministic
  multi-start greedy search plus coordinate refinement;
- reports complete, infeasible_fixed_constraints, or search_exhausted without
  turning a partial result into a false success;
- independently validates route connectivity, edge containment, trace-to-trace
  and trace-to-pad clearance, metrics, and exact user-approved jumpers;
- serializes every input and result with an explicit schema version.

Run the engine against the checked-in fixture:

~~~bash
cargo run --manifest-path crates/layout-core/Cargo.toml \
  --example place_json -- \
  crates/layout-core/fixtures/two-pin.json -
~~~

Exit codes are 0 for a complete placement, 2 for infeasible fixed constraints,
3 for an exhausted search, and 1 for invalid input or I/O.

This is a correctness-oriented baseline, not the final optimizer. The next
placement engine can replace the heuristic behind place while preserving the
model, validator, fixtures, metrics, deterministic seeds, and failure states.
