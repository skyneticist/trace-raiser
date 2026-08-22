import type { GeneratorSettings, Pad, ParsedBoard, Point, Trace } from "./pcb-core";

export type WidthSample = { t: number; width: number };
export type CenterSample = WidthSample & { point: Point };

export type TraceProfile = {
  trace: Trace;
  kind: "trace" | "corner";
  length: number;
  trunkWidth: number;
  neckWidth: number;
  startExit: number | null;
  endExit: number | null;
  startShoulder: number | null;
  endShoulder: number | null;
  style: GeneratorSettings["trace_style"];
  teardropLength: number;
  lobes: Point[][];
  samples: WidthSample[];
  centerline: CenterSample[];
};

const EPSILON = 1e-6;
const RAY_EPSILON = 1e-8;
const LEGACY_ATTACHMENT_TOLERANCE = 1e-5;

export function buildTraceProfiles(board: ParsedBoard, settings: GeneratorSettings): TraceProfile[] {
  const pads = board.pads.filter(isSupportedBackPad);
  const traces = board.traces.filter((trace) => trace.layer === "B.Cu");
  const trims = traces.map(() => ({ start: 0, end: 0 }));
  const corners = settings.width_mode === "auto" && settings.trace_style === "vintage"
    ? buildCornerProfiles(board, traces, settings, trims)
    : [];
  const straight = traces.map((trace, index) => buildTraceProfile(trace, pads, settings, trims[index]));
  return [...straight, ...corners];
}

export function buildTraceProfile(
  trace: Trace,
  pads: Pad[],
  settings: GeneratorSettings,
  trim = { start: 0, end: 0 },
): TraceProfile {
  const dx = trace.end.x - trace.start.x;
  const dy = trace.end.y - trace.start.y;
  const sourceLength = Math.hypot(dx, dy);
  const direction = sourceLength > EPSILON
    ? { x: dx / sourceLength, y: dy / sourceLength }
    : { x: 1, y: 0 };
  const startTrim = clamp(trim.start, 0, sourceLength * 0.45);
  const endTrim = clamp(trim.end, 0, sourceLength * 0.45);
  const start = {
    x: trace.start.x + direction.x * startTrim,
    y: trace.start.y + direction.y * startTrim,
  };
  const end = {
    x: trace.end.x - direction.x * endTrim,
    y: trace.end.y - direction.y * endTrim,
  };
  const length = Math.max(0, sourceLength - startTrim - endTrim);
  const trunkWidth = settings.width_mode === "preserve"
    ? trace.width
    : Math.max(trace.width, settings.trace_width);
  const neckWidth = settings.width_mode === "preserve"
    ? trunkWidth
    : Math.min(trunkWidth, settings.neckdown_width);

  if (length <= EPSILON || settings.width_mode === "preserve") {
    const samples = [{ t: 0, width: trunkWidth }, { t: 1, width: trunkWidth }];
    return {
      trace, kind: "trace", length, trunkWidth, neckWidth,
      startExit: null, endExit: null, startShoulder: null, endShoulder: null,
      style: settings.trace_style, teardropLength: settings.teardrop_length, lobes: [], samples,
      centerline: samples.map((sample) => ({ ...sample, point: mixPoint(start, end, sample.t) })),
    };
  }

  const startAttachment = startTrim <= EPSILON
    ? matchingPadAttachment(trace, pads, trace.start, direction)
    : null;
  const endAttachment = endTrim <= EPSILON
    ? matchingPadAttachment(trace, pads, trace.end, { x: -direction.x, y: -direction.y })
    : null;
  const [startLobeLength, endLobeLength] = effectiveTeardropLengths(
    sourceLength,
    startAttachment,
    endAttachment,
    settings,
  );
  const startExit = startAttachment ? startAttachment.exit + startLobeLength : null;
  const endExit = endAttachment ? endAttachment.exit + endLobeLength : null;
  const startShoulder = null;
  const endShoulder = null;
  const lobes: Point[][] = [];
  if (startAttachment && startLobeLength > 0) {
    const lobe = teardropLobePolygon(
      trace.start,
      direction,
      startAttachment,
      neckWidth,
      startLobeLength,
      settings.teardrop_strength,
    );
    if (lobe) lobes.push(lobe);
  }
  if (endAttachment && endLobeLength > 0) {
    const lobe = teardropLobePolygon(
      trace.end,
      { x: -direction.x, y: -direction.y },
      endAttachment,
      neckWidth,
      endLobeLength,
      settings.teardrop_strength,
    );
    if (lobe) lobes.push(lobe);
  }
  const distances = new Set([0, length]);
  const taperLength = Math.max(settings.taper_length, EPSILON);

  if (startExit !== null) {
    distances.add(clamp(startExit, 0, length));
    distances.add(clamp(startExit + taperLength, 0, length));
  }
  if (endExit !== null) {
    distances.add(clamp(length - endExit, 0, length));
    distances.add(clamp(length - endExit - taperLength, 0, length));
  }
  if (startExit !== null && endExit !== null) {
    const intersection = (length - endExit + startExit) / 2;
    const startRampEnd = startExit + taperLength;
    const endRampStart = length - endExit - taperLength;
    if (intersection >= startExit - EPSILON && intersection <= startRampEnd + EPSILON
      && intersection >= endRampStart - EPSILON && intersection <= length - endExit + EPSILON) {
      distances.add(clamp(intersection, 0, length));
    }
  }

  if (settings.trace_style !== "technical") {
    const steps = Math.min(192, Math.max(2, Math.ceil(length / 0.45)));
    for (let index = 0; index <= steps; index += 1) distances.add(length * index / steps);
  }

  const profile: TraceProfile = {
    trace, kind: "trace", length, trunkWidth, neckWidth, startExit, endExit,
    startShoulder, endShoulder, style: settings.trace_style,
    teardropLength: settings.teardrop_length, lobes, samples: [], centerline: [],
  };
  profile.samples = [...distances]
    .sort((a, b) => a - b)
    .map((distance) => ({ t: distance / length, width: widthAtDistance(profile, distance, taperLength) }));
  profile.centerline = profile.samples.map((sample) => ({
    ...sample,
    point: mixPoint(start, end, sample.t),
  }));
  return profile;
}

export function widthAt(profile: TraceProfile, t: number, taperLength: number) {
  return widthAtDistance(profile, clamp(t, 0, 1) * profile.length, Math.max(taperLength, EPSILON));
}

function widthAtDistance(profile: TraceProfile, distance: number, taperLength: number) {
  const allowance = (fromExit: number, travelled: number) => {
    const progress = clamp((travelled - fromExit) / taperLength, 0, 1);
    const eased = profile.style === "technical" ? progress : smootherstep(progress);
    return profile.neckWidth + (profile.trunkWidth - profile.neckWidth) * eased;
  };
  const candidates = [];
  if (profile.startExit !== null) candidates.push(allowance(profile.startExit, distance));
  if (profile.endExit !== null) candidates.push(allowance(profile.endExit, profile.length - distance));
  return candidates.reduce((selected, candidate) => {
    const selectedChange = Math.abs(selected - profile.trunkWidth);
    const candidateChange = Math.abs(candidate - profile.trunkWidth);
    return candidateChange > selectedChange + EPSILON
      || Math.abs(candidateChange - selectedChange) <= EPSILON && candidate < selected
      ? candidate
      : selected;
  }, profile.trunkWidth);
}

export function traceProfilePolygon(profile: TraceProfile, extraWidth = 0, capSteps = 8): Point[] {
  if (profile.length <= EPSILON || profile.centerline.length < 2) return [];
  const tangentAt = (index: number) => {
    const before = profile.centerline[Math.max(0, index - 1)].point;
    const after = profile.centerline[Math.min(profile.centerline.length - 1, index + 1)].point;
    return normalize({ x: after.x - before.x, y: after.y - before.y });
  };
  const left = profile.centerline.map((sample, index) => {
    const tangent = tangentAt(index);
    const normal = { x: -tangent.y, y: tangent.x };
    const radius = (sample.width + extraWidth) / 2;
    return { x: sample.point.x + normal.x * radius, y: sample.point.y + normal.y * radius };
  });
  const right = [...profile.centerline].reverse().map((sample, reverseIndex) => {
    const index = profile.centerline.length - 1 - reverseIndex;
    const tangent = tangentAt(index);
    const normal = { x: -tangent.y, y: tangent.x };
    const radius = (sample.width + extraWidth) / 2;
    return { x: sample.point.x - normal.x * radius, y: sample.point.y - normal.y * radius };
  });
  const startTangent = tangentAt(0);
  const endTangent = tangentAt(profile.centerline.length - 1);
  const startTheta = Math.atan2(startTangent.y, startTangent.x);
  const endTheta = Math.atan2(endTangent.y, endTangent.x);
  const endRadius = (profile.centerline.at(-1)!.width + extraWidth) / 2;
  const startRadius = (profile.centerline[0].width + extraWidth) / 2;
  const endCap = arcPoints(profile.centerline.at(-1)!.point, endRadius, endTheta + Math.PI / 2, endTheta - Math.PI / 2, capSteps);
  const startCap = arcPoints(profile.centerline[0].point, startRadius, startTheta - Math.PI / 2, startTheta - Math.PI * 1.5, capSteps);
  return [...left, ...endCap.slice(1), ...right.slice(1), ...startCap.slice(1)];
}

export function traceProfilePolygons(profile: TraceProfile, extraWidth = 0): Point[][] {
  const main = traceProfilePolygon(profile, extraWidth);
  return [main, ...profile.lobes].filter((polygon) => polygon.length >= 3);
}

type TraceTrim = { start: number; end: number };
type NodeEntry = { traceIndex: number; atStart: boolean; node: Point; other: Point; length: number };

function buildCornerProfiles(
  board: ParsedBoard,
  traces: Trace[],
  settings: GeneratorSettings,
  trims: TraceTrim[],
) {
  const nodes = new Map<string, NodeEntry[]>();
  traces.forEach((trace, traceIndex) => {
    const length = Math.hypot(trace.end.x - trace.start.x, trace.end.y - trace.start.y);
    if (length <= EPSILON) return;
    for (const [node, other, atStart] of [
      [trace.start, trace.end, true],
      [trace.end, trace.start, false],
    ] as const) {
      const key = pointKey(node);
      const entries = nodes.get(key) ?? [];
      entries.push({ traceIndex, atStart, node, other, length });
      nodes.set(key, entries);
    }
  });

  const corners: TraceProfile[] = [];
  for (const entries of nodes.values()) {
    if (entries.length !== 2 || protectedNode(entries[0].node, board)) continue;
    const [first, second] = entries;
    const firstTrace = traces[first.traceIndex];
    const secondTrace = traces[second.traceIndex];
    if (!sameCircuit(firstTrace, secondTrace)) continue;
    const firstDirection = normalize({ x: first.other.x - first.node.x, y: first.other.y - first.node.y });
    const secondDirection = normalize({ x: second.other.x - second.node.x, y: second.other.y - second.node.y });
    const interior = Math.acos(clamp(dot(firstDirection, secondDirection), -1, 1));
    const deflection = Math.PI - interior;
    if (deflection < Math.PI / 36 || deflection > Math.PI * 5 / 6) continue;

    const firstWidth = effectiveTraceWidth(firstTrace, settings);
    const secondWidth = effectiveTraceWidth(secondTrace, settings);
    const wanted = settings.corner_radius * Math.tan(deflection / 2);
    const reach = Math.min(wanted, first.length * 0.34, second.length * 0.34);
    if (reach < Math.max(firstWidth, secondWidth) * 0.3) continue;

    const start = add(first.node, scale(firstDirection, reach));
    const end = add(second.node, scale(secondDirection, reach));
    if (quadraticMinimumRadius(start, first.node, end) < Math.max(firstWidth, secondWidth) * 0.55) continue;

    if (first.atStart) trims[first.traceIndex].start = Math.max(trims[first.traceIndex].start, reach);
    else trims[first.traceIndex].end = Math.max(trims[first.traceIndex].end, reach);
    if (second.atStart) trims[second.traceIndex].start = Math.max(trims[second.traceIndex].start, reach);
    else trims[second.traceIndex].end = Math.max(trims[second.traceIndex].end, reach);

    const steps = Math.min(24, Math.max(8, Math.ceil(deflection / (Math.PI / 24))));
    const centerline: CenterSample[] = [];
    for (let index = 0; index <= steps; index += 1) {
      const t = index / steps;
      centerline.push({
        t,
        point: quadratic(start, first.node, end, t),
        width: firstWidth + (secondWidth - firstWidth) * smootherstep(t),
      });
    }
    const length = centerline.slice(1).reduce(
      (total, sample, index) => total + distance(centerline[index].point, sample.point),
      0,
    );
    corners.push({
      trace: { ...firstTrace, start, end, width: Math.min(firstTrace.width, secondTrace.width) },
      kind: "corner", length, trunkWidth: Math.max(firstWidth, secondWidth),
      neckWidth: Math.min(firstWidth, secondWidth), startExit: null, endExit: null,
      startShoulder: null, endShoulder: null, style: "vintage",
      teardropLength: settings.teardrop_length, lobes: [],
      samples: centerline.map(({ t, width }) => ({ t, width })), centerline,
    });
  }
  return corners;
}

function protectedNode(point: Point, board: ParsedBoard) {
  return board.pads.some((pad) => isSupportedBackPad(pad) && pointInPolygon(point, padPolygon(pad)))
    || board.vias.some((via) => (via.layers.includes("B.Cu") || via.layers.includes("*.Cu"))
      && Math.hypot(point.x - via.position.x, point.y - via.position.y) <= via.size / 2 + EPSILON);
}

function effectiveTraceWidth(trace: Trace, settings: GeneratorSettings) {
  return settings.width_mode === "preserve" ? trace.width : Math.max(trace.width, settings.trace_width);
}

function sameCircuit(first: Trace, second: Trace) {
  const firstNet = first.net_id ?? null;
  const secondNet = second.net_id ?? null;
  return firstNet !== null && firstNet > 0 ? firstNet === secondNet : firstNet === null && secondNet === null;
}

export function padPolygon(pad: Pad): Point[] {
  if (pad.shape === "circle") return circle(pad.position, Math.min(pad.size.x, pad.size.y) / 2, 24);
  if (pad.shape === "oval") {
    const horizontal = pad.size.x >= pad.size.y;
    const width = horizontal ? pad.size.y : pad.size.x;
    const halfSpan = Math.abs(pad.size.x - pad.size.y) / 2;
    const localA = horizontal ? { x: -halfSpan, y: 0 } : { x: 0, y: -halfSpan };
    const localB = horizontal ? { x: halfSpan, y: 0 } : { x: 0, y: halfSpan };
    const a = rotateLocal(localA, pad.position, pad.rotation);
    const b = rotateLocal(localB, pad.position, pad.rotation);
    return capsule(a, b, width, 16);
  }
  return [
    { x: -pad.size.x / 2, y: -pad.size.y / 2 },
    { x: pad.size.x / 2, y: -pad.size.y / 2 },
    { x: pad.size.x / 2, y: pad.size.y / 2 },
    { x: -pad.size.x / 2, y: pad.size.y / 2 },
  ].map((point) => rotateLocal(point, pad.position, pad.rotation));
}

export function padExitDistance(pad: Pad, direction: Point, origin = pad.position) {
  const polygon = padPolygon(pad);
  let nearest = Number.POSITIVE_INFINITY;
  for (let index = 0; index < polygon.length; index += 1) {
    const a = polygon[index];
    const b = polygon[(index + 1) % polygon.length];
    const edge = { x: b.x - a.x, y: b.y - a.y };
    const offset = { x: a.x - origin.x, y: a.y - origin.y };
    const denominator = cross(direction, edge);
    if (Math.abs(denominator) <= EPSILON) continue;
    const distance = cross(offset, edge) / denominator;
    const edgeT = cross(offset, direction) / denominator;
    if (distance >= -RAY_EPSILON && edgeT >= -RAY_EPSILON && edgeT <= 1 + RAY_EPSILON) {
      const positiveDistance = Math.max(0, distance);
      if (positiveDistance > RAY_EPSILON) nearest = Math.min(nearest, positiveDistance);
    }
  }
  return Number.isFinite(nearest) ? nearest : null;
}

function isTaperAttachment(trace: Trace, pad: Pad, endpoint: Point) {
  const traceNet = trace.net_id ?? 0;
  const padNet = pad.net_id ?? 0;
  if (traceNet > 0 && traceNet === padNet) return pointInPolygon(endpoint, padPolygon(pad));
  return trace.net_id == null && pad.net_id == null
    && Math.hypot(endpoint.x - pad.position.x, endpoint.y - pad.position.y) < LEGACY_ATTACHMENT_TOLERANCE;
}

function matchingPadAttachment(trace: Trace, pads: Pad[], endpoint: Point, direction: Point) {
  const attachments = pads
    .filter((pad) => isTaperAttachment(trace, pad, endpoint))
    .map((pad) => ({ pad, exit: padExitDistance(pad, direction, endpoint) }))
    .filter((attachment): attachment is { pad: Pad; exit: number } => attachment.exit !== null);
  return attachments.sort((a, b) => b.exit - a.exit)[0] ?? null;
}

type PadAttachment = NonNullable<ReturnType<typeof matchingPadAttachment>>;

function effectiveTeardropLengths(
  traceLength: number,
  start: PadAttachment | null,
  end: PadAttachment | null,
  settings: GeneratorSettings,
): [number, number] {
  if (settings.trace_style !== "vintage" || settings.teardrop_strength <= EPSILON) return [0, 0];
  const usable = Math.max(0, traceLength - (start?.exit ?? 0) - (end?.exit ?? 0));
  const requested = Math.max(0, settings.teardrop_length);
  let lengths: [number, number];
  if (start && end) {
    const each = Math.min(requested, usable * 0.48);
    lengths = [each, each];
  } else if (start) {
    lengths = [Math.min(requested, usable), 0];
  } else if (end) {
    lengths = [0, Math.min(requested, usable)];
  } else {
    lengths = [0, 0];
  }
  return lengths.map((length) => length >= 0.25 ? length : 0) as [number, number];
}

function rayExitHit(origin: Point, direction: Point, polygon: Point[]) {
  let nearest: { distance: number; point: Point; tangent: Point } | null = null;
  for (let index = 0; index < polygon.length; index += 1) {
    const a = polygon[index];
    const b = polygon[(index + 1) % polygon.length];
    const edge = { x: b.x - a.x, y: b.y - a.y };
    const offset = { x: a.x - origin.x, y: a.y - origin.y };
    const denominator = cross(direction, edge);
    if (Math.abs(denominator) <= EPSILON) continue;
    const distance = cross(offset, edge) / denominator;
    const edgeT = cross(offset, direction) / denominator;
    if (distance <= RAY_EPSILON || edgeT < -RAY_EPSILON || edgeT > 1 + RAY_EPSILON) continue;
    if (!nearest || distance < nearest.distance) {
      nearest = {
        distance,
        point: {
          x: origin.x + direction.x * distance,
          y: origin.y + direction.y * distance,
        },
        tangent: normalize(edge),
      };
    }
  }
  return nearest;
}

function teardropLobePolygon(
  endpoint: Point,
  direction: Point,
  attachment: PadAttachment,
  neckWidth: number,
  length: number,
  strength: number,
) {
  if (length < 0.25 || strength <= EPSILON) return null;
  const polygon = padPolygon(attachment.pad);
  const normal = { x: -direction.y, y: direction.x };
  const available = Math.min(attachment.pad.size.x, attachment.pad.size.y) * 0.95;
  const shoulderWidth = neckWidth + Math.max(0, available - neckWidth) * clamp(strength, 0, 1);
  const shoulderHalf = shoulderWidth / 2;
  const shoulderOrigin = (sign: number) => ({
    x: endpoint.x + normal.x * shoulderHalf * sign,
    y: endpoint.y + normal.y * shoulderHalf * sign,
  });
  const upper = rayExitHit(shoulderOrigin(1), direction, polygon);
  const lower = rayExitHit(shoulderOrigin(-1), direction, polygon);
  if (!upper || !lower) return null;
  const tipCenter = {
    x: endpoint.x + direction.x * (attachment.exit + length),
    y: endpoint.y + direction.y * (attachment.exit + length),
  };
  const upperTip = { x: tipCenter.x + normal.x * neckWidth / 2, y: tipCenter.y + normal.y * neckWidth / 2 };
  const lowerTip = { x: tipCenter.x - normal.x * neckWidth / 2, y: tipCenter.y - normal.y * neckWidth / 2 };
  const curve = (start: NonNullable<ReturnType<typeof rayExitHit>>, end: Point, side: number) => {
    const chord = { x: end.x - start.point.x, y: end.y - start.point.y };
    const tangent = dot(start.tangent, chord) >= 0
      ? start.tangent
      : { x: -start.tangent.x, y: -start.tangent.y };
    const span = Math.hypot(chord.x, chord.y);
    const sideCoordinate = (point: Point) => dot(point, normal) * side;
    const inwardSlope = -dot(tangent, normal) * side;
    const sideDrop = Math.max(0, sideCoordinate(start.point) - sideCoordinate(end));
    const monotonicLimit = inwardSlope > EPSILON ? sideDrop * 0.9 / inwardSlope : span * 0.34;
    const shoulderHandle = Math.min(span * 0.34, Math.max(0, monotonicLimit));
    const control1 = {
      x: start.point.x + tangent.x * shoulderHandle,
      y: start.point.y + tangent.y * shoulderHandle,
    };
    const control2 = {
      x: end.x - direction.x * span * 0.30,
      y: end.y - direction.y * span * 0.30,
    };
    return Array.from({ length: 13 }, (_, index) => (
      cubicBezier(start.point, control1, control2, end, index / 12)
    ));
  };
  const shape = [...curve(upper, upperTip, 1), ...curve(lower, lowerTip, -1).reverse()];
  return Math.abs(polygonArea(shape)) > EPSILON ? shape : null;
}

export function isSupportedBackPad(pad: Pad) {
  const kind = pad.pad_type ?? pad.pad_kind ?? pad.kind ?? (pad.drill !== null ? "thru_hole" : "smd");
  return kind === "thru_hole" && pad.shape !== "custom"
    && (pad.layers.includes("B.Cu") || pad.layers.includes("*.Cu"));
}

function rotateLocal(point: Point, center: Point, degrees: number) {
  const angle = degrees * Math.PI / 180;
  const sin = Math.sin(angle);
  const cos = Math.cos(angle);
  return { x: center.x + point.x * cos - point.y * sin, y: center.y + point.x * sin + point.y * cos };
}

function circle(center: Point, radius: number, steps: number) {
  return Array.from({ length: steps }, (_, index) => {
    const angle = Math.PI * 2 * index / steps;
    return { x: center.x + Math.cos(angle) * radius, y: center.y + Math.sin(angle) * radius };
  });
}

function capsule(start: Point, end: Point, width: number, steps: number) {
  const theta = Math.atan2(end.y - start.y, end.x - start.x);
  const half = Math.max(4, Math.floor(steps / 2));
  return [
    ...arcPoints(end, width / 2, theta - Math.PI / 2, theta + Math.PI / 2, half),
    ...arcPoints(start, width / 2, theta + Math.PI / 2, theta + Math.PI * 1.5, half),
  ];
}

function arcPoints(center: Point, radius: number, start: number, end: number, steps: number) {
  return Array.from({ length: steps + 1 }, (_, index) => {
    const angle = start + (end - start) * index / steps;
    return { x: center.x + Math.cos(angle) * radius, y: center.y + Math.sin(angle) * radius };
  });
}

function quadratic(start: Point, control: Point, end: Point, t: number) {
  const inverse = 1 - t;
  return {
    x: inverse * inverse * start.x + 2 * inverse * t * control.x + t * t * end.x,
    y: inverse * inverse * start.y + 2 * inverse * t * control.y + t * t * end.y,
  };
}

function cubicBezier(start: Point, control1: Point, control2: Point, end: Point, t: number) {
  const inverse = 1 - t;
  return {
    x: inverse ** 3 * start.x + 3 * inverse * inverse * t * control1.x
      + 3 * inverse * t * t * control2.x + t ** 3 * end.x,
    y: inverse ** 3 * start.y + 3 * inverse * inverse * t * control1.y
      + 3 * inverse * t * t * control2.y + t ** 3 * end.y,
  };
}

function polygonArea(points: Point[]) {
  return points.reduce((area, point, index) => {
    const next = points[(index + 1) % points.length];
    return area + point.x * next.y - next.x * point.y;
  }, 0) / 2;
}

function quadraticMinimumRadius(start: Point, control: Point, end: Point) {
  const first = { x: control.x - start.x, y: control.y - start.y };
  const second = {
    x: end.x - 2 * control.x + start.x,
    y: end.y - 2 * control.y + start.y,
  };
  let minimum = Number.POSITIVE_INFINITY;
  for (let index = 0; index <= 24; index += 1) {
    const t = index / 24;
    const derivative = {
      x: 2 * (first.x + second.x * t),
      y: 2 * (first.y + second.y * t),
    };
    const acceleration = { x: 2 * second.x, y: 2 * second.y };
    const numerator = Math.pow(Math.hypot(derivative.x, derivative.y), 3);
    const denominator = Math.abs(cross(derivative, acceleration));
    if (denominator > EPSILON) minimum = Math.min(minimum, numerator / denominator);
  }
  return minimum;
}

function pointKey(point: Point) {
  return `${Math.round(point.x * 10_000)},${Math.round(point.y * 10_000)}`;
}

function normalize(point: Point) {
  const length = Math.hypot(point.x, point.y);
  return length <= EPSILON ? { x: 1, y: 0 } : { x: point.x / length, y: point.y / length };
}

function mixPoint(start: Point, end: Point, t: number) {
  return { x: start.x + (end.x - start.x) * t, y: start.y + (end.y - start.y) * t };
}

function smootherstep(value: number) {
  const t = clamp(value, 0, 1);
  return t * t * t * (t * (t * 6 - 15) + 10);
}

function add(a: Point, b: Point) { return { x: a.x + b.x, y: a.y + b.y }; }
function scale(point: Point, value: number) { return { x: point.x * value, y: point.y * value }; }
function dot(a: Point, b: Point) { return a.x * b.x + a.y * b.y; }
function distance(a: Point, b: Point) { return Math.hypot(a.x - b.x, a.y - b.y); }
function cross(a: Point, b: Point) { return a.x * b.y - a.y * b.x; }
function clamp(value: number, min: number, max: number) { return Math.max(min, Math.min(max, value)); }

function pointInPolygon(point: Point, polygon: Point[]) {
  let inside = false;
  for (let index = 0, previous = polygon.length - 1; index < polygon.length; previous = index, index += 1) {
    const a = polygon[previous];
    const b = polygon[index];
    if ((a.y > point.y) !== (b.y > point.y)
      && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x) inside = !inside;
  }
  return inside;
}
