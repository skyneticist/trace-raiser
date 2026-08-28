# Copperline AutoLayout foundation

## Product contract

The first product slice is intentionally narrow and serious:

- input is a KiCad PCB whose footprints are already assigned;
- components are front-side through-hole parts with explicit courtyard-derived
  placement envelopes;
- copper is routed on B.Cu;
- the board has one simple outer outline;
- locked parts, legal rotations, placement keepouts, edge clearance, trace
  width, and trace clearance are hard constraints;
- jumpers are forbidden unless a user accepts a concrete proposal, producing a
  stable approval ID and a bounded count;
- every result is deterministic for a design, engine version, options, and
  seed;
- incomplete work is returned as incomplete work, never as success.

SMD, multiple copper layers, blind/buried vias, differential pairs, RF,
impedance control, length matching, high voltage, and safety-critical design
are outside v1.

## Architecture

1. kicad-layout parses bounded KiCad S-expressions and retains original byte
   spans. It patches only a selected footprint's immediate (at ...) node.
2. Its design bridge converts nets, pads, footprints, and Edge.Cuts into
   layout-core. It requires explicit footprint envelopes and rejects unsupported
   geometry rather than estimating bodies from pad extents.
3. layout-core owns the versioned design, placement, routing, diagnostics,
   metrics, and jumper-approval contracts.
4. Optimizers produce candidates; independent validators decide whether those
   candidates satisfy the canonical contract.
5. The KiCad adapter emits localized changes. KiCad parsing and DRC remain the
   final external authority.

The boundary is designed so a stronger placer or router can be substituted
without changing file handling, evaluation fixtures, or UI semantics.

## Placement path

The checked-in engine is a deterministic legality-first baseline:

- place locked parts and prove their mutual legality;
- order movable parts by connectivity and envelope area;
- search legal grid poses across allowed rotations;
- minimize estimated half-perimeter wire length with a small displacement
  penalty;
- use deterministic seeded restarts;
- refine complete candidates coordinate-by-coordinate;
- retain the best complete candidate, or the best evidenced partial candidate
  if the search budget is exhausted.

The next quality step is a two-stage global placer: simulated annealing over
continuous/discrete poses, followed by the existing exact legalizer and local
refinement. The baseline remains useful as a fallback and regression oracle.

## Router decision

The product router should be native to the canonical model:

- coarse global routing and ordering;
- grid-based A* or Lee search with 45-degree support;
- multi-terminal nets decomposed into a deterministic Steiner approximation;
- negotiated congestion with rip-up and reroute;
- cost terms for length, bends, narrow channels, pad escape, and future jumper
  proposals;
- a final geometry pass and independent connectivity/clearance validation;
- explicit partial status when one or more nets remain unrouted.

[Freerouting](https://github.com/freerouting/freerouting) is valuable as an
offline benchmark oracle because it has a mature DSN-to-autorouter-to-SES
pipeline. It is not the embedded product engine: its GPL-3.0, Java, and
Specctra boundary and its general multilayer assumptions do not match a small
local Rust/WASM single-layer engine with reviewed jumpers.

KiCad's
[IPC API](https://dev-docs.kicad.org/en/apis-and-binding/ipc-api/for-addon-developers/)
is the preferred optional desktop integration. It is intended as a stable,
language-agnostic interface, but KiCad 9 and 10 require a running GUI; headless
IPC arrives in KiCad 11. File import/export therefore remains the portable
baseline.

## Evidence and quality gates

Every candidate release must meet all of these gates:

1. Parser no-op output is byte-identical.
2. A localized placement changes only authorized footprint pose spans.
3. Repeated runs with the same seed are byte-identical.
4. Invalid designs, infeasible fixed constraints, exhausted searches, and
   complete solutions are distinguishable.
5. The independent validator reports no overlap, outline, keepout, connectivity,
   clearance, metric, or jumper-authority defects.
6. kicad-cli pcb drc --format json --exit-code-violations reports zero
   violations on every emitted corpus board.
7. Placement and routing are measured on a checked-in corpus, recording
   completion rate, HPWL, routed-net rate, trace length, bend count, jumper
   count, candidate evaluations, wall time, and deterministic replay hash.
8. Freerouting is run out-of-process on compatible corpus cases to establish a
   quality reference, not to define correctness.

The official KiCad CLI documents JSON DRC output and an exit code on
violations: <https://docs.kicad.org/master/en/cli/cli.html>.

## Current validation boundary

The repository currently proves the canonical model, deterministic placement,
route validation, bounded parsing, no-op preservation, localized placement
patching, explicit-envelope conversion, and straight-outline assembly with Rust
tests and Clippy warnings-as-errors.

kicad-cli is not installed in the present development environment. Real KiCad
parse/DRC evidence is therefore pending; green host tests must not be described
as KiCad approval.

## Near-term sequence

1. Add courtyard extraction (F.CrtYd) and arc-aware Edge.Cuts assembly.
2. Add a corpus manifest and JSON evaluator with golden replay hashes.
3. Implement the global-placement stage behind the existing contract.
4. Implement a deterministic single-net A* router, then negotiated rip-up and
   reroute.
5. Add proposed-jumper review and immutable approval IDs.
6. Emit tracks through a syntax-preserving KiCad patcher and gate them with
   kicad-cli DRC.
7. Add preview/compare/accept UI only after corpus gates are automated.
