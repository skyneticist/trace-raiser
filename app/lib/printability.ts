import type { CopperZone, GeneratorSettings, Pad, ParsedBoard, Point, Trace, Via } from "./pcb-core";
import { buildTraceProfiles, padPolygon, traceProfilePolygons } from "./manufacturing-geometry.ts";

export type ClearanceConflict = {
  kind: "trace-trace" | "trace-pad" | "trace-via" | "trace-zone" | "pad-pad" | "pad-via" | "pad-zone" | "via-via" | "via-zone" | "zone-zone";
  location: Point;
  /** Physical edge-to-edge gap. Negative values mean copper overlaps. */
  gap: number;
  requiredClearance: number;
  firstNet: number | null;
  secondNet: number | null;
  firstNetName: string | null;
  secondNetName: string | null;
};

type Closest = { a: Point; b: Point; ta: number; tb: number; distance: number };
type FeatureKind = "trace" | "pad" | "via" | "zone";
type FeatureSource = Trace | Pad | Via | CopperZone;
type ClearanceFeature = {
  kind: FeatureKind;
  shapes: Point[][];
  anchors: Point[];
  source: FeatureSource;
  netId: number | null;
};

export function findClearanceConflicts(
  board: ParsedBoard,
  settings: GeneratorSettings,
): ClearanceConflict[] {
  const profiles = buildTraceProfiles(board, settings);
  const pads = board.pads.filter(isBackCopperPad);
  const vias = board.vias.filter(isBackCopperVia);
  const zones = (board.zones ?? []).filter((zone) => (
    zone.layer === "B.Cu"
    && zone.kind !== "keepout"
    && zone.polygons.length > 0
    && !(zone.kind === "teardrop" && settings.width_mode === "auto" && settings.trace_style === "vintage")
  ));
  const padPolygons = pads.map((pad) => padPolygon(pad));
  const viaPolygons = vias.map((via) => circle(via.position, via.size / 2, 24));
  const features: ClearanceFeature[] = [
    ...profiles.map((profile): ClearanceFeature => ({
      kind: "trace",
      shapes: traceProfilePolygons(profile),
      anchors: [profile.centerline[0]?.point, profile.centerline.at(-1)?.point].filter((point): point is Point => Boolean(point)),
      source: profile.trace,
      netId: positiveNetId(profile.trace.net_id),
    })),
    ...pads.map((pad, index): ClearanceFeature => ({
      kind: "pad", shapes: [padPolygons[index]], anchors: [pad.position], source: pad,
      netId: positiveNetId(pad.net_id),
    })),
    ...vias.map((via, index): ClearanceFeature => ({
      kind: "via", shapes: [viaPolygons[index]], anchors: [via.position], source: via,
      netId: positiveNetId(via.net_id),
    })),
    ...zones.map((zone): ClearanceFeature => ({
      kind: "zone", shapes: zone.polygons, anchors: zone.polygons.flat(), source: zone,
      netId: positiveNetId(zone.net_id),
    })),
  ];

  // Netless legacy boards need the same transitive, authored-anchor
  // connectivity used by the Rust exporter. Pairwise checks incorrectly split
  // one circuit at every rounded corner, pad, or filled zone.
  const parent = features.map((_, index) => index);
  const root = (index: number): number => {
    while (parent[index] !== index) {
      parent[index] = parent[parent[index]];
      index = parent[index];
    }
    return index;
  };
  const union = (first: number, second: number) => {
    const firstRoot = root(first);
    const secondRoot = root(second);
    if (firstRoot !== secondRoot) parent[secondRoot] = firstRoot;
  };
  for (let first = 0; first < features.length; first += 1) {
    for (let second = first + 1; second < features.length; second += 1) {
      const sameKnownNet = features[first].netId !== null && features[first].netId === features[second].netId;
      const connectedUnknown = features[first].netId === null && features[second].netId === null
        && featuresCoordinateConnected(features[first], features[second]);
      if (sameKnownNet || connectedUnknown) union(first, second);
    }
  }

  const conflicts: ClearanceConflict[] = [];
  for (let first = 0; first < features.length; first += 1) {
    for (let second = first + 1; second < features.length; second += 1) {
      if (root(first) === root(second)) continue;
      const a = features[first];
      const b = features[second];
      if (boundingShapeGap(a.shapes, b.shapes) > settings.trace_clearance + 1e-6) continue;
      const closest = closestShapes(a.shapes, b.shapes);
      if (violatesClearance(closest.gap, settings.trace_clearance)) {
        conflicts.push(conflict(conflictKind(a.kind, b.kind), closest, a.source, b.source, settings));
      }
    }
  }

  return conflicts.sort((a, b) => (a.gap - a.requiredClearance) - (b.gap - b.requiredClearance));
}

function conflict(
  kind: ClearanceConflict["kind"],
  closest: ReturnType<typeof emptyClosest>,
  first: Trace | Pad | Via | CopperZone,
  second: Trace | Pad | Via | CopperZone,
  settings: GeneratorSettings,
): ClearanceConflict {
  return {
    kind,
    location: { x: (closest.a.x + closest.b.x) / 2, y: (closest.a.y + closest.b.y) / 2 },
    gap: closest.gap,
    requiredClearance: settings.trace_clearance,
    firstNet: positiveNetId(first.net_id),
    secondNet: positiveNetId(second.net_id),
    firstNetName: first.net_name ?? null,
    secondNetName: second.net_name ?? null,
  };
}

function closestPolygons(first: Point[], second: Point[]) {
  let best = emptyClosest();
  if (first.some((point) => pointInPolygon(point, second)) || second.some((point) => pointInPolygon(point, first))) {
    const point = first.find((candidate) => pointInPolygon(candidate, second))
      ?? second.find((candidate) => pointInPolygon(candidate, first))!;
    return { a: point, b: point, ta: 0, tb: 0, distance: 0, gap: 0 };
  }
  for (let a = 0; a < first.length; a += 1) {
    for (let b = 0; b < second.length; b += 1) {
      const closest = closestSegments(first[a], first[(a + 1) % first.length], second[b], second[(b + 1) % second.length]);
      if (closest.distance < best.gap) best = { ...closest, gap: closest.distance };
    }
  }
  return best;
}

function closestShapes(first: Point[][], second: Point[][]) {
  let best = emptyClosest();
  for (const firstPolygon of first) {
    for (const secondPolygon of second) {
      const closest = closestPolygons(firstPolygon, secondPolygon);
      if (closest.gap < best.gap) best = closest;
    }
  }
  return best;
}

function boundingGap(first: Point[], second: Point[]) {
  const bounds = (polygon: Point[]) => polygon.reduce((result, point) => ({
    minX: Math.min(result.minX, point.x),
    minY: Math.min(result.minY, point.y),
    maxX: Math.max(result.maxX, point.x),
    maxY: Math.max(result.maxY, point.y),
  }), { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity });
  const a = bounds(first);
  const b = bounds(second);
  const dx = Math.max(0, a.minX - b.maxX, b.minX - a.maxX);
  const dy = Math.max(0, a.minY - b.maxY, b.minY - a.maxY);
  return Math.hypot(dx, dy);
}

function boundingShapeGap(first: Point[][], second: Point[][]) {
  let minimum = Number.POSITIVE_INFINITY;
  for (const firstPolygon of first) {
    for (const secondPolygon of second) minimum = Math.min(minimum, boundingGap(firstPolygon, secondPolygon));
  }
  return minimum;
}

function closestSegments(a0: Point, a1: Point, b0: Point, b1: Point): Closest {
  const u = subtract(a1, a0);
  const v = subtract(b1, b0);
  const w = subtract(a0, b0);
  const aa = dot(u, u);
  const bb = dot(u, v);
  const cc = dot(v, v);
  const dd = dot(u, w);
  const ee = dot(v, w);
  const denominator = aa * cc - bb * bb;
  let ta = denominator < 1e-12 ? 0 : clamp((bb * ee - cc * dd) / denominator, 0, 1);
  let tb = cc < 1e-12 ? 0 : clamp((bb * ta + ee) / cc, 0, 1);
  ta = aa < 1e-12 ? 0 : clamp((bb * tb - dd) / aa, 0, 1);
  tb = cc < 1e-12 ? 0 : clamp((bb * ta + ee) / cc, 0, 1);
  const a = { x: mix(a0.x, a1.x, ta), y: mix(a0.y, a1.y, ta) };
  const b = { x: mix(b0.x, b1.x, tb), y: mix(b0.y, b1.y, tb) };
  return { a, b, ta, tb, distance: distance(a, b) };
}

function pointInPolygon(point: Point, polygon: Point[]) {
  let inside = false;
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i, i += 1) {
    const a = polygon[i];
    const b = polygon[j];
    if ((a.y > point.y) !== (b.y > point.y)
      && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x) inside = !inside;
  }
  return inside;
}

function isBackCopperPad(pad: Pad) {
  const kind = pad.pad_type ?? pad.pad_kind ?? pad.kind ?? (pad.drill !== null ? "thru_hole" : "smd");
  return kind === "thru_hole" && pad.shape !== "custom"
    && (pad.layers.includes("B.Cu") || pad.layers.includes("*.Cu"));
}

function isBackCopperVia(via: Via) {
  return via.layers.includes("B.Cu") || via.layers.includes("*.Cu");
}

function pointInShape(point: Point, polygons: Point[][]) {
  return polygons.some((polygon) => pointInPolygon(point, polygon) || pointOnPolygon(point, polygon));
}
function pointOnPolygon(point: Point, polygon: Point[]) {
  return polygon.some((start, index) => pointSegmentDistance(point, start, polygon[(index + 1) % polygon.length]) <= 1e-6);
}
function pointSegmentDistance(point: Point, start: Point, end: Point) {
  const edge = subtract(end, start);
  const lengthSquared = dot(edge, edge);
  if (lengthSquared <= 1e-12) return distance(point, start);
  const t = clamp(dot(subtract(point, start), edge) / lengthSquared, 0, 1);
  return distance(point, { x: mix(start.x, end.x, t), y: mix(start.y, end.y, t) });
}
function featuresCoordinateConnected(first: ClearanceFeature, second: ClearanceFeature) {
  return first.anchors.some((point) => pointInShape(point, second.shapes))
    || second.anchors.some((point) => pointInShape(point, first.shapes));
}
function positiveNetId(value?: number | null) {
  return typeof value === "number" && Number.isInteger(value) && value > 0 ? value : null;
}
function conflictKind(first: FeatureKind, second: FeatureKind): ClearanceConflict["kind"] {
  const order: FeatureKind[] = ["trace", "pad", "via", "zone"];
  const pair = order.indexOf(first) <= order.indexOf(second) ? `${first}-${second}` : `${second}-${first}`;
  return pair as ClearanceConflict["kind"];
}
function violatesClearance(gap: number, required: number) {
  return gap < 1e-6 || gap + 1e-6 < required;
}
function emptyClosest() { return { a: { x: 0, y: 0 }, b: { x: 0, y: 0 }, ta: 0, tb: 0, distance: Infinity, gap: Infinity }; }
function circle(center: Point, radius: number, steps: number) { return Array.from({ length: steps }, (_, index) => { const angle = Math.PI * 2 * index / steps; return { x: center.x + Math.cos(angle) * radius, y: center.y + Math.sin(angle) * radius }; }); }
function subtract(a: Point, b: Point) { return { x: a.x - b.x, y: a.y - b.y }; }
function dot(a: Point, b: Point) { return a.x * b.x + a.y * b.y; }
function distance(a: Point, b: Point) { return Math.hypot(a.x - b.x, a.y - b.y); }
function mix(a: number, b: number, t: number) { return a + (b - a) * t; }
function clamp(value: number, min: number, max: number) { return Math.max(min, Math.min(max, value)); }
