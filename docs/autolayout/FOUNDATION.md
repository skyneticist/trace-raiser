# Copperline AutoLayout foundation

## Product contract

The first product slice is intentionally narrow and serious:

- input is a KiCad PCB whose footprints are already assigned;
- components are front-side through-hole parts with valid F.CrtYd geometry or
  a trusted manual placement envelope;
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
2. Its design bridge converts nets, pads, footprints, courtyards, and Edge.Cuts
   into layout-core. It extracts conservative courtyard envelopes and rejects
   unsupported geometry rather than estimating bodies from pad extents.
3. layout-core owns the versioned design, placement, routing, diagnostics,
   metrics, and jumper-approval contracts.
4. Optimizers produce candidates; independent validators decide whether those
   candidates satisfy the canonical contract.
5. The KiCad adapter emits localized changes. KiCad parsing and DRC remain the
   final external authority.

The boundary is designed so a stronger placer or router can be substituted
without changing file handling, evaluation fixtures, or UI semantics.

## KiCad geometry ingestion contract

- F.CrtYd and B.CrtYd lines, rectangles, polygons, circles, and arcs are parsed
  in footprint-local coordinates. F.CrtYd is authoritative for a front-side
  footprint; a valid B.CrtYd expands the same conservative envelope for
  underside hardware.
- Courtyard line/arc endpoints use KiCad's 0.02 mm chaining tolerance and snap
  to deterministic midpoints; their analytic envelope still includes every
  authored endpoint. Board-outline endpoints must coincide on KiCad's
  0.000001 mm grid; only sub-grid floating tolerance is used.
- A trusted manual envelope may replace a missing F.CrtYd, but it cannot shrink
  a parsed courtyard. Courtyard centerline bounds are analytic; reducing them to
  one local AABB is conservative and is not exact shape-aware collision.
- Edge.Cuts accepts one rectangle (including a nonzero corner radius), one
  polygon, one circle, or one closed chain of unordered/reversed lines and arcs.
  Multiple board loops, holes, branches, footprint-owned cutouts, malformed
  curves, and Bézier geometry fail explicitly.
- Circular geometry is tessellated with at most 0.005 mm sagitta, quantized to
  KiCad's 0.000001 mm coordinate grid, canonicalized for replay, and limited to
  4096 final vertices. A 0.005003 mm approximation guard is added to placement
  and routing edge clearances whenever the outline contains curves.
- Both legacy ordinal-plus-name nets and KiCad 10 name-based pad nets are
  retained. Malformed required graphic coordinates/children and malformed net
  records fail parsing; unsupported authoritative geometry is not silently
  discarded.

The curve tolerance follows KiCad 10's own
[courtyard-cache implementation](https://gitlab.com/kicad/code/kicad/-/blob/10.0/pcbnew/footprint.cpp#L3424),
and canonical coordinates follow KiCad's documented
[one-nanometer board resolution](https://dev-docs.kicad.org/en/file-formats/sexpr-intro/#_board_coordinates).

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
6. kicad-cli pcb drc --format json --severity-all --exit-code-violations
   reports zero violations on every emitted corpus board.
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
patching, automatic courtyard conversion, KiCad 10 name-based net conversion,
and canonical straight/arc/circle/rounded-rectangle outline assembly with Rust
tests and Clippy warnings-as-errors.

The corpus evaluator produces a JSON report with placement status, HPWL,
displacement, candidate count, elapsed time, and a stable FNV-1a replay
fingerprint. Its routine smoke corpus is deliberately limited to one modern
KiCad 10 success case and one unsupported-geometry rejection case. More cases
are added only for concrete regressions or during a later dedicated test pass.

KiCad CLI 10.0.5 has also parsed and strictly checked both the routed
public/sample-sensor.kicad_pcb fixture with F.CrtYd rectangles and the separate
mixed line/arc curved-outline contract fixture, with all severities enabled:
zero violations and zero unconnected items. Keeping curved-outline coverage in
the adapter fixture avoids claiming arc support in the Studio's separate
printable-geometry core. scripts/check-autolayout.sh runs both DRC checks when
KiCad is discoverable; set REQUIRE_KICAD_DRC=1 to make a missing CLI a hard
failure. KICAD_CLI can select a non-standard executable path, and the standard
macOS application path is discovered automatically.

This is real KiCad evidence for the checked fixture, not general approval of
future placement or routing output. Every emitted corpus candidate must still
pass the same strict gate.

## Near-term sequence

1. Implement the global-placement stage behind the existing contract.
2. Implement a deterministic single-net A* router, then negotiated rip-up and
   reroute.
3. Add proposed-jumper review and immutable approval IDs.
4. Emit tracks through a syntax-preserving KiCad patcher and gate them with
   kicad-cli DRC.
5. Add preview/compare/accept UI only after corpus gates are automated.
