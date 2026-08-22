import type { CopperZone, GeneratorSettings, Pad, ParsedBoard, Point, Trace, Via } from "./pcb-core";
import { buildTraceProfiles, padPolygon, traceProfilePolygons, type TraceProfile } from "./manufacturing-geometry.ts";

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

export function findClearanceConflicts(
  board: ParsedBoard,
  settings: GeneratorSettings,
): ClearanceConflict[] {
  const profiles = buildTraceProfiles(board, settings);
  const profilePolygons = profiles.map((profile) => traceProfilePolygons(profile));
  const conflicts: ClearanceConflict[] = [];

  for (let first = 0; first < profiles.length; first += 1) {
    for (let second = first + 1; second < profiles.length; second += 1) {
      if (sameProfileCircuit(profiles[first], profilePolygons[first], profiles[second], profilePolygons[second])) continue;
      if (boundingShapeGap(profilePolygons[first], profilePolygons[second]) > settings.trace_clearance + 1e-6) continue;
      const closest = closestShapes(profilePolygons[first], profilePolygons[second]);
      if (violatesClearance(closest.gap, settings.trace_clearance)) {
        conflicts.push(conflict("trace-trace", closest, profiles[first].trace, profiles[second].trace, settings));
      }
    }
  }

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
  for (let profileIndex = 0; profileIndex < profiles.length; profileIndex += 1) {
    const profile = profiles[profileIndex];
    const profilePolygon = profilePolygons[profileIndex];
    for (let padIndex = 0; padIndex < pads.length; padIndex += 1) {
      const pad = pads[padIndex];
      const polygon = padPolygons[padIndex];
      if (sameProfileFeatureCircuit(profile, profilePolygon, pad, polygon)) continue;
      if (boundingShapeGap(profilePolygon, [polygon]) > settings.trace_clearance + 1e-6) continue;
      const closest = closestShapes(profilePolygon, [polygon]);
      if (violatesClearance(closest.gap, settings.trace_clearance)) conflicts.push(conflict("trace-pad", closest, profile.trace, pad, settings));
    }
    for (let viaIndex = 0; viaIndex < vias.length; viaIndex += 1) {
      const via = vias[viaIndex];
      const polygon = viaPolygons[viaIndex];
      if (sameProfileFeatureCircuit(profile, profilePolygon, via, polygon)) continue;
      if (boundingShapeGap(profilePolygon, [polygon]) > settings.trace_clearance + 1e-6) continue;
      const closest = closestShapes(profilePolygon, [polygon]);
      if (violatesClearance(closest.gap, settings.trace_clearance)) conflicts.push(conflict("trace-via", closest, profile.trace, via, settings));
    }
  }

  for (let first = 0; first < pads.length; first += 1) {
    for (let second = first + 1; second < pads.length; second += 1) {
      const firstPolygon = padPolygons[first];
      const secondPolygon = padPolygons[second];
      if (sameFeatureCircuit(pads[first], firstPolygon, pads[second], secondPolygon)) continue;
      if (boundingGap(firstPolygon, secondPolygon) > settings.trace_clearance + 1e-6) continue;
      const closest = closestPolygons(firstPolygon, secondPolygon);
      if (violatesClearance(closest.gap, settings.trace_clearance)) conflicts.push(conflict("pad-pad", closest, pads[first], pads[second], settings));
    }
    for (let viaIndex = 0; viaIndex < vias.length; viaIndex += 1) {
      const via = vias[viaIndex];
      const padShape = padPolygons[first];
      const viaShape = viaPolygons[viaIndex];
      if (sameFeatureCircuit(pads[first], padShape, via, viaShape)) continue;
      if (boundingGap(padShape, viaShape) > settings.trace_clearance + 1e-6) continue;
      const closest = closestPolygons(padShape, viaShape);
      if (violatesClearance(closest.gap, settings.trace_clearance)) conflicts.push(conflict("pad-via", closest, pads[first], via, settings));
    }
  }

  for (let first = 0; first < vias.length; first += 1) {
    for (let second = first + 1; second < vias.length; second += 1) {
      const firstShape = circle(vias[first].position, vias[first].size / 2, 24);
      const secondShape = circle(vias[second].position, vias[second].size / 2, 24);
      if (sameFeatureCircuit(vias[first], firstShape, vias[second], secondShape)) continue;
      const delta = distance(vias[first].position, vias[second].position);
      const measuredGap = delta - vias[first].size / 2 - vias[second].size / 2;
      if (violatesClearance(measuredGap, settings.trace_clearance)) {
        const closest = {
          a: vias[first].position,
          b: vias[second].position,
          ta: 0,
          tb: 0,
          distance: delta,
          gap: measuredGap,
        };
        conflicts.push(conflict("via-via", closest, vias[first], vias[second], settings));
      }
    }
  }

  for (let zoneIndex = 0; zoneIndex < zones.length; zoneIndex += 1) {
    const zone = zones[zoneIndex];
    for (let profileIndex = 0; profileIndex < profiles.length; profileIndex += 1) {
      const profile = profiles[profileIndex];
      if (sameNet(zone.net_id, profile.trace.net_id)) continue;
      if (boundingShapeGap(zone.polygons, profilePolygons[profileIndex]) > settings.trace_clearance + 1e-6) continue;
      const closest = closestShapes(zone.polygons, profilePolygons[profileIndex]);
      if (violatesClearance(closest.gap, settings.trace_clearance)) {
        conflicts.push(conflict("trace-zone", closest, profile.trace, zone, settings));
      }
    }
    for (let padIndex = 0; padIndex < pads.length; padIndex += 1) {
      if (sameNet(zone.net_id, pads[padIndex].net_id)) continue;
      if (boundingShapeGap(zone.polygons, [padPolygons[padIndex]]) > settings.trace_clearance + 1e-6) continue;
      const closest = closestShapes(zone.polygons, [padPolygons[padIndex]]);
      if (violatesClearance(closest.gap, settings.trace_clearance)) {
        conflicts.push(conflict("pad-zone", closest, pads[padIndex], zone, settings));
      }
    }
    for (let viaIndex = 0; viaIndex < vias.length; viaIndex += 1) {
      if (sameNet(zone.net_id, vias[viaIndex].net_id)) continue;
      if (boundingShapeGap(zone.polygons, [viaPolygons[viaIndex]]) > settings.trace_clearance + 1e-6) continue;
      const closest = closestShapes(zone.polygons, [viaPolygons[viaIndex]]);
      if (violatesClearance(closest.gap, settings.trace_clearance)) {
        conflicts.push(conflict("via-zone", closest, vias[viaIndex], zone, settings));
      }
    }
    for (let other = zoneIndex + 1; other < zones.length; other += 1) {
      if (sameNet(zone.net_id, zones[other].net_id)) continue;
      if (boundingShapeGap(zone.polygons, zones[other].polygons) > settings.trace_clearance + 1e-6) continue;
      const closest = closestShapes(zone.polygons, zones[other].polygons);
      if (violatesClearance(closest.gap, settings.trace_clearance)) {
        conflicts.push(conflict("zone-zone", closest, zone, zones[other], settings));
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
    firstNet: first.net_id ?? null,
    secondNet: second.net_id ?? null,
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

function sameNet(a?: number | null, b?: number | null) { return a != null && a > 0 && a === b; }
function pointInShape(point: Point, polygons: Point[][]) {
  return polygons.some((polygon) => pointInPolygon(point, polygon));
}
function sameProfileCircuit(a: TraceProfile, aPolygon: Point[][], b: TraceProfile, bPolygon: Point[][]) {
  if (sameNet(a.trace.net_id, b.trace.net_id)) return true;
  if (a.trace.net_id != null || b.trace.net_id != null) return false;
  return [a.trace.start, a.trace.end].some((point) => pointInShape(point, bPolygon))
    || [b.trace.start, b.trace.end].some((point) => pointInShape(point, aPolygon));
}
function sameProfileFeatureCircuit(profile: TraceProfile, profilePolygon: Point[][], feature: Pad | Via, featurePolygon: Point[]) {
  if (sameNet(profile.trace.net_id, feature.net_id)) return true;
  if (profile.trace.net_id != null || feature.net_id != null) return false;
  return [profile.trace.start, profile.trace.end].some((point) => pointInPolygon(point, featurePolygon))
    || pointInShape(feature.position, profilePolygon);
}
function sameFeatureCircuit(a: Pad | Via, aPolygon: Point[], b: Pad | Via, bPolygon: Point[]) {
  if (sameNet(a.net_id, b.net_id)) return true;
  return a.net_id == null && b.net_id == null
    && (pointInPolygon(a.position, bPolygon) || pointInPolygon(b.position, aPolygon));
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
