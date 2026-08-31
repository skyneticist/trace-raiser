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
6. A dedicated browser WASM boundary runs the same placer, router, validator,
   and syntax-preserving emitter. The Studio keeps its result as a proposal
   until an explicit accept-and-download action.

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

The checked-in engine is a deterministic two-stage placer:

- place locked parts and prove their mutual legality;
- keep restart zero as the legality-first baseline and regression oracle;
- on later restarts, run bounded seeded simulated annealing over grid positions,
  legal rotations, and component-position swaps;
- score the global state by HPWL, displacement, and explicit soft penalties for
  outline, keepout, and component-clearance violations;
- use the global result only as a hint for the exact grid legalizer;
- refine every complete candidate coordinate-by-coordinate without the hint;
- compare final candidates on the canonical HPWL/displacement objective, so a
  weaker hinted candidate cannot displace the baseline;
- return the best evidenced partial candidate if the search budget is exhausted.

The annealing budget is derived from the existing placement search budget and
capped at 2,048 proposals per hinted restart. One restart therefore preserves
the original baseline behavior; two or more enable global placement without an
options-schema change.

## Routing path

The checked-in router is native to the canonical model:

- bounded eight-neighbor A* supplies orthogonal and 45-degree grid moves;
- exact terminal escapes allow pads that do not fall on the routing grid;
- multi-terminal nets grow a deterministic tree and may attach to projections
  on existing same-net segments;
- every move uses the independent validator's conservative board-edge, pad,
  NPTH, and different-net trace-clearance geometry;
- bend and historical-congestion costs guide the search;
- failed nets are promoted on the next pass, all prior copper is ripped up, and
  bounded seeded passes explore alternate orders and corridors;
- the best candidate maximizes routed nets, then minimizes trace length and
  bend count;
- exhausted nets are returned explicitly in `unrouted_net_ids`, never reported
  as routed.

## KiCad candidate emission

The adapter now turns a complete canonical route into a localized KiCad patch:

- it reconstructs the design from the same parsed board before writing, so a
  route validated against different authoritative geometry or connectivity is
  rejected;
- it patches only immediate footprint poses and inserts deterministic B.Cu
  `segment` nodes before the root closing parenthesis, without reserializing the
  rest of the document;
- canonical segment ordering and SHA-256-derived UUIDv8 identifiers make
  same-input replay byte-identical while avoiding existing board UUIDs;
- legacy ordinal nets emit as `(net N)`, while KiCad 10 name-only nets remain
  quoted and escaped as `(net "NAME")`;
- existing segments, track arcs, vias, and zones are never overwritten or
  implicitly merged;
- partial routes and jumper-bearing routes cannot enter the copper-emission
  path, and the emitted document must reparse with the exact expected segment
  count before it is returned.

The checked smoke starts from the unrouted KiCad 10 corpus board, performs
placement and routing, emits the candidate twice to require byte identity, then
runs strict KiCad DRC on those generated bytes.

The browser boundary applies hard option budgets, rejects existing copper,
forbids jumpers, and returns only a complete route with a SHA-256 proposal ID.
Its output is reparsed by the Studio's independent printable-geometry WASM
before it can be previewed.

## Jumper authority path

Jumper review is an explicit, content-bound transaction:

- `propose_jumpers` inspects each unrouted net's actual conductive components
  and proposes a deterministic minimum-length spanning set between terminal
  representatives;
- a proposal contains geometry, estimated wire length, and a stable proposal
  ID, but it grants no routing authority;
- `accept_jumper_proposal` is the only operation that mints an approval ID and
  returns a consistent updated design and route;
- the SHA-256 approval ID binds an approval-independent design snapshot, exact
  placement, net, and complete direction-independent jumper set;
- one approval is bounded to its exact jumper count, and proposals exceeding 32
  jumpers fail closed instead of creating an unreviewable batch;
- the independent route validator recomputes the content identity whenever an
  approval is used, rejecting copied IDs, changed endpoints, subsets, or added
  jumpers.

Changing unrelated approvals does not invalidate an accepted jumper, but any
electrical-design, board/component-geometry, placement, or routing-rule change
does. The proposal artifact is therefore reviewable before acceptance and
tamper-evident after it.

The ID is a content hash, not a signature or proof of human identity; the UI
must invoke acceptance only from an explicit user action and retain the reviewed
proposal for audit.

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

The repository currently proves the canonical model, deterministic two-stage
placement with exact legalization, deterministic A* routing with bounded
rip-up/reroute, content-bound jumper proposals and explicit acceptance,
independent route validation, bounded parsing, no-op preservation, localized
placement and B.Cu segment patching, source-bound design reconstruction,
automatic courtyard conversion, KiCad 10 name-based net conversion, and
canonical straight/arc/circle/rounded-rectangle outline assembly with Rust
tests and Clippy warnings-as-errors. The same candidate path is compiled into a
separate browser WASM module and exposed through a review-first Studio flow with
original/proposed comparison, replay metrics, explicit acceptance, and locked
printable export until KiCad DRC and re-import.

The corpus evaluator produces a JSON report with placement status, HPWL,
displacement, candidate count, elapsed time, and a stable FNV-1a replay
fingerprint. Its routine smoke corpus is deliberately limited to one modern
KiCad 10 success case and one unsupported-geometry rejection case. More cases
are added only for concrete regressions or during a later dedicated test pass.
One separate routing smoke routes twice around an NPTH obstacle, requires
byte-identical replay, and passes the independent connectivity and clearance
validator. It is intentionally one case rather than a broad routing test matrix.
The existing jumper-authority contract smoke now also covers deterministic
proposal replay, explicit acceptance, forbidden pre-acceptance use, and
geometry-tamper rejection; no separate jumper test matrix was added.

KiCad CLI 10.0.5 has parsed and strictly checked the routed
public/sample-sensor.kicad_pcb fixture, the mixed line/arc curved-outline
contract fixture, and the candidate generated from the unrouted KiCad 10
name-net corpus board, with all severities enabled: zero violations and zero
unconnected items. Keeping curved-outline coverage in the adapter fixture
avoids claiming arc support in the Studio's separate printable-geometry core.
scripts/check-autolayout.sh generates the route during the gate and runs all
three DRC checks when KiCad is discoverable; set REQUIRE_KICAD_DRC=1 to make a
missing CLI a hard failure. KICAD_CLI can select a non-standard executable
path, and the standard macOS application path is discovered automatically.

The browser assessment fixture adds a larger end-to-end checkpoint without
turning the routine suite into a broad test matrix: 10 through-hole footprints,
12 two-terminal nets, three locked edge connectors, one locked series part,
four movable parts, and two fixed NPTH mounting obstacles. With the checked
Balanced assessment preset and seed 424243, the shipped WASM produces proposal
`sha256:430c6a6bdb2b8640027c0e249cf635b30f7d1d97ff53fdd63717d023b97dade6`:
all 12 nets routed in 50 segments with 38 bends and 278.5524033593761 mm of
trace. KiCad CLI 10.0.5 strict DRC reported zero violations and zero unconnected
items on those exact candidate bytes.

This is real KiCad evidence for the exact generated smoke candidate, not
general approval of arbitrary future output. The assessment result likewise
proves only that one deterministic fixture and preset. Every emitted candidate
must still pass the same strict gate before use.

## Delivered product slice

The planned foundation milestones now terminate in a usable browser workflow:
load an eligible unrouted KiCad board, tune bounded constraints, create a local
proposal, compare original and proposed geometry, and explicitly download an
accepted candidate. Copperline does not claim browser-side DRC certification;
the user must run KiCad DRC, save, and re-import the checked board before
printable export is unlocked. Physical jumper serialization, broader component
classes, and multilayer routing are separate future capabilities, not implied
by this slice.
