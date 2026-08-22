import assert from "node:assert/strict";
import test from "node:test";
import { findClearanceConflicts } from "../app/lib/printability.ts";
import { DEFAULT_SETTINGS } from "../app/lib/settings.ts";

const trace = (y, net) => ({
  start: { x: 0, y }, end: { x: 20, y }, width: 2.5,
  layer: "B.Cu", net_id: net, net_name: `N${net}`,
});
const board = (traces, pads = [], vias = []) => ({
  name: "test", bounds: { min_x: 0, min_y: 0, max_x: 20, max_y: 20, width: 20, height: 20 },
  outline: [{ x: 0, y: -5 }, { x: 20, y: -5 }, { x: 20, y: 20 }, { x: 0, y: 20 }],
  traces, pads, vias, warnings: [], stats: { traces: traces.length, pads: pads.length, vias: vias.length, holes: 0 },
});

test("allows same-net joins and overlaps but flags a different-net crossing location", () => {
  const same = findClearanceConflicts(board([trace(0, 1), { ...trace(0, 1), start: { x: 10, y: -5 }, end: { x: 10, y: 5 } }]), DEFAULT_SETTINGS);
  assert.equal(same.length, 0);

  const different = findClearanceConflicts(board([trace(0, 1), { ...trace(0, 2), start: { x: 10, y: -5 }, end: { x: 10, y: 5 } }]), DEFAULT_SETTINGS);
  assert.equal(different.length, 1);
  assert.equal(different[0].kind, "trace-trace");
  assert.ok(different[0].location.x > 8 && different[0].location.x < 12);
  assert.ok(different[0].location.y > -2 && different[0].location.y < 2);
});

test("checks different-net pads and vias while allowing same-net attachments", () => {
  const pad = { position: { x: 0, y: 0 }, size: { x: 4, y: 4 }, drill: 1,
    pad_type: "thru_hole", shape: "circle", rotation: 0, layers: ["*.Cu"], net_id: 1, net_name: "N1" };
  const via = { position: { x: 10, y: 0 }, size: 3, drill: 1, layers: ["B.Cu"], net_id: 2, net_name: "N2" };
  const conflicts = findClearanceConflicts(board([trace(0, 1)], [pad], [via]), DEFAULT_SETTINGS);
  assert.equal(conflicts.some((item) => item.kind === "trace-pad"), false);
  assert.equal(conflicts.some((item) => item.kind === "trace-via"), true);
  assert.ok(conflicts.find((item) => item.kind === "trace-via").location.x > 8);
});

test("uses coordinate-connected fallback only for legacy features with both nets missing", () => {
  const legacyTrace = { ...trace(0, 1), net_id: undefined, net_name: undefined };
  const legacyPad = { position: { x: 0, y: 0 }, size: { x: 4, y: 4 }, drill: 1,
    pad_type: "thru_hole", shape: "circle", rotation: 0, layers: ["*.Cu"] };
  const joined = findClearanceConflicts(board([legacyTrace], [legacyPad]), DEFAULT_SETTINGS);
  assert.equal(joined.length, 0);

  const unknownNearby = { ...legacyPad, position: { x: 5, y: 2.6 } };
  const conservative = findClearanceConflicts(board([legacyTrace], [unknownNearby]), DEFAULT_SETTINGS);
  assert.equal(conservative.some((item) => item.kind === "trace-pad"), true);
});

test("allows a coordinate-connected legacy T-junction but rejects an unanchored crossing", () => {
  const horizontal = { ...trace(0, 1), net_id: undefined, net_name: undefined };
  const anchored = {
    ...horizontal,
    start: { x: 10, y: -5 },
    end: { x: 10, y: 0 },
  };
  assert.equal(findClearanceConflicts(board([horizontal, anchored]), DEFAULT_SETTINGS).length, 0);

  const crossing = { ...anchored, end: { x: 10, y: 5 } };
  assert.equal(
    findClearanceConflicts(board([horizontal, crossing]), DEFAULT_SETTINGS)
      .some((item) => item.kind === "trace-trace"),
    true,
  );
});

test("blocks overlap and point tangency even when requested clearance is zero", () => {
  const featurePad = (x, net) => ({ position: { x, y: 10 }, size: { x: 2, y: 2 }, drill: 0.8,
    pad_type: "thru_hole", shape: "circle", rotation: 0, layers: ["*.Cu"], net_id: net, net_name: `N${net}` });
  const zeroClearance = { ...DEFAULT_SETTINGS, trace_clearance: 0 };
  assert.equal(findClearanceConflicts(board([], [featurePad(5, 1), featurePad(6, 2)]), zeroClearance).length, 1);
  assert.equal(findClearanceConflicts(board([], [featurePad(5, 1), featurePad(7, 2)]), zeroClearance).length, 1);
});

test("checks every different-net pad/via pairing and reports physical gap separately", () => {
  const featurePad = (x, net) => ({ position: { x, y: 10 }, size: { x: 2, y: 2 }, drill: 0.8,
    pad_type: "thru_hole", shape: "circle", rotation: 0, layers: ["*.Cu"], net_id: net, net_name: `N${net}` });
  const pads = [featurePad(0, 1), featurePad(2.3, 2), featurePad(10, 3)];
  const vias = [
    { position: { x: 11.2, y: 10 }, size: 1, drill: 0.4, layers: ["B.Cu"], net_id: 4, net_name: "N4" },
    { position: { x: 18, y: 10 }, size: 1, drill: 0.4, layers: ["B.Cu"], net_id: 5, net_name: "N5" },
    { position: { x: 19.2, y: 10 }, size: 1, drill: 0.4, layers: ["B.Cu"], net_id: 6, net_name: "N6" },
  ];
  const conflicts = findClearanceConflicts(board([], pads, vias), DEFAULT_SETTINGS);
  assert.ok(conflicts.some((item) => item.kind === "pad-pad"));
  assert.ok(conflicts.some((item) => item.kind === "pad-via"));
  assert.ok(conflicts.some((item) => item.kind === "via-via"));
  const padPair = conflicts.find((item) => item.kind === "pad-pad");
  assert.ok(Math.abs(padPair.gap - 0.3) < 0.02);
  assert.equal(padPair.requiredClearance, 0.5);
});

test("checks generated copper against exact B.Cu zone fills", () => {
  const featurePad = { position: { x: 6.3, y: 2.5 }, size: { x: 2, y: 2 }, drill: 0.8,
    pad_type: "thru_hole", shape: "circle", rotation: 0, layers: ["*.Cu"], net_id: 2, net_name: "N2" };
  const zone = { layer: "B.Cu", kind: "copper", net_id: 1, net_name: "GND", polygons: [[
    { x: 0, y: 0 }, { x: 5, y: 0 }, { x: 5, y: 5 }, { x: 0, y: 5 },
  ]] };
  const different = { ...board([], [featurePad]), zones: [zone] };
  assert.equal(findClearanceConflicts(different, DEFAULT_SETTINGS).some((item) => item.kind === "pad-zone"), true);

  const same = { ...different, pads: [{ ...featurePad, net_id: 1, net_name: "GND" }] };
  assert.equal(findClearanceConflicts(same, DEFAULT_SETTINGS).some((item) => item.kind === "pad-zone"), false);
});
