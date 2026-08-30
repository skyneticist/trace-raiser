import assert from "node:assert/strict";
import test from "node:test";
import {
  buildTraceProfile,
  buildTraceProfiles,
  padExitDistance,
  traceProfilePolygons,
  widthAt,
} from "../app/lib/manufacturing-geometry.ts";
import { DEFAULT_SETTINGS } from "../app/lib/settings.ts";

const TECHNICAL_SETTINGS = { ...DEFAULT_SETTINGS, trace_style: "technical" };

const pad = (x, net = 1) => ({
  position: { x, y: 0 }, size: { x: 4, y: 4 }, drill: 1.2,
  pad_type: "thru_hole", shape: "circle", rotation: 0,
  layers: ["*.Cu"], net_id: net, net_name: `N${net}`,
});
const trace = (width = 3) => ({
  start: { x: 0, y: 0 }, end: { x: 20, y: 0 }, width,
  layer: "B.Cu", net_id: 1, net_name: "N1",
});

test("keeps the neck narrow through the pad and tapers after the exact pad exit", () => {
  const profile = buildTraceProfile(trace(), [pad(0)], TECHNICAL_SETTINGS);
  assert.equal(padExitDistance(pad(0), { x: 1, y: 0 }), 2);
  assert.equal(widthAt(profile, 0.1, 4), 1.4);
  assert.equal(widthAt(profile, 0.2, 4), 1.4 + (3 - 1.4) * 0.5);
  assert.equal(widthAt(profile, 0.3, 4), 3);
});

test("widens a uniform source into a trunk while retaining local pad tapers", () => {
  const profile = buildTraceProfile(trace(2.5), [pad(0), pad(20)], {
    ...TECHNICAL_SETTINGS,
    trace_width: 3,
    neckdown_width: 1.4,
  });
  assert.equal(profile.trunkWidth, 3);
  assert.equal(profile.samples[0].width, 1.4);
  assert.equal(profile.samples.at(-1).width, 1.4);
  assert.equal(widthAt(profile, 0.5, 4), 3);
});

test("inserts the min-envelope intersection when two short pad tapers overlap", () => {
  const shortTrace = { ...trace(2.5), end: { x: 8, y: 0 } };
  const profile = buildTraceProfile(shortTrace, [pad(0), pad(8)], {
    ...TECHNICAL_SETTINGS,
    trace_width: 3,
  });
  assert.ok(profile.samples.some((sample) => Math.abs(sample.t - 0.5) < 1e-9));
  assert.equal(widthAt(profile, 0.5, 4), 2.2);
});

test("preserve mode retains the authored width and disables tapers", () => {
  const profile = buildTraceProfile(trace(2.2), [pad(0), pad(20)], {
    ...TECHNICAL_SETTINGS,
    width_mode: "preserve",
  });
  assert.equal(profile.trunkWidth, 2.2);
  assert.deepEqual(profile.samples.map((sample) => sample.width), [2.2, 2.2]);
});

test("uses the farthest exit when overlapping same-net pads contain an endpoint", () => {
  const small = { ...pad(0), size: { x: 2, y: 2 } };
  const large = { ...pad(0), size: { x: 6, y: 6 } };
  const profile = buildTraceProfile(trace(), [small, large], TECHNICAL_SETTINGS);
  assert.ok(Math.abs(profile.startExit - 3) < 1e-9);
});

test("uses zero-slope easing for soft tapers", () => {
  const profile = buildTraceProfile(trace(), [pad(0)], {
    ...DEFAULT_SETTINGS,
    trace_style: "soft",
  });
  const quarterRamp = widthAt(profile, 0.15, 4);
  const linearQuarter = 1.4 + (3 - 1.4) * 0.25;
  assert.ok(quarterRamp < linearQuarter);
  assert.ok(quarterRamp > 1.4);
});

test("vintage teardrops widen at the pad shoulder and return smoothly to the route", () => {
  const profile = buildTraceProfile(trace(), [pad(0)], {
    ...DEFAULT_SETTINGS,
    trace_width: 3,
    trace_style: "vintage",
    teardrop_length: 3,
    teardrop_strength: 0.75,
  });
  assert.equal(profile.lobes.length, 1);
  assert.equal(widthAt(profile, 0.1, 4), profile.neckWidth);
  assert.ok(widthAt(profile, 0.3, 4) > profile.neckWidth);
  assert.ok(traceProfilePolygons(profile).flat().every((point) => Number.isFinite(point.x) && Number.isFinite(point.y)));
  const lobe = profile.lobes[0];
  const upper = lobe.slice(0, 13);
  const widths = [2.15, 2.5, 3.0, 3.5, 4.0, 4.75].map((x) => {
    const ys = lobe.filter((point) => Math.abs(point.x - x) < 0.22).map((point) => Math.abs(point.y));
    return Math.max(...ys);
  });
  assert.ok(widths.every((width, index) => index === 0 || width <= widths[index - 1] + 0.08));
  assert.ok(upper[1].x > upper[0].x && upper.at(-1).x > upper[1].x);
});

test("treats KiCad net zero as netless for authored pad attachments", () => {
  const zeroTrace = { ...trace(), net_id: 0, net_name: "" };
  const zeroPad = { ...pad(0, 0), net_name: "" };
  const profile = buildTraceProfile(zeroTrace, [zeroPad], DEFAULT_SETTINGS);
  assert.ok(profile.startExit > 2);
  assert.equal(profile.lobes.length, 1);
});

test("smoothly combines overlapping two-ended vintage tapers at the midpoint", () => {
  const profile = buildTraceProfile(trace(), [pad(0), pad(20)], {
    ...DEFAULT_SETTINGS,
    trace_width: 3,
    neckdown_width: 1.4,
    taper_length: 12,
    teardrop_length: 3,
    trace_style: "vintage",
  });
  const step = 0.0001;
  const left = widthAt(profile, 0.5 - step, 12);
  const middle = widthAt(profile, 0.5, 12);
  const right = widthAt(profile, 0.5 + step, 12);
  const leftSlope = (middle - left) / (profile.length * step);
  const rightSlope = (right - middle) / (profile.length * step);

  assert.ok([left, middle, right].every(Number.isFinite));
  assert.ok(middle >= left && middle >= right);
  assert.ok(Math.abs(left - right) < 1e-9);
  assert.ok(Math.abs(leftSlope) < 0.01, `left midpoint slope was ${leftSlope}`);
  assert.ok(Math.abs(rightSlope) < 0.01, `right midpoint slope was ${rightSlope}`);
});

test("carries a pad taper's width slope smoothly through a right-angle vintage bend", () => {
  const board = {
    name: "tapered-corner",
    bounds: { min_x: 0, min_y: 0, max_x: 30, max_y: 30, width: 30, height: 30 },
    outline: [{ x: 0, y: 0 }, { x: 30, y: 0 }, { x: 30, y: 30 }, { x: 0, y: 30 }],
    traces: [
      { ...trace(0.6), start: { x: 2, y: 10 }, end: { x: 13, y: 10 } },
      { ...trace(0.6), start: { x: 13, y: 10 }, end: { x: 13, y: 25 } },
    ],
    pads: [{
      position: { x: 2, y: 10 }, size: { x: 4, y: 4 }, drill: 1.2,
      pad_type: "thru_hole", shape: "circle", rotation: 0,
      layers: ["*.Cu"], net_id: 1, net_name: "N1",
    }],
    vias: [], warnings: [], stats: { traces: 2, pads: 1, vias: 0, holes: 1 },
  };
  const profiles = buildTraceProfiles(board, { ...DEFAULT_SETTINGS, corner_radius: 3 });
  const straight = profiles.find((profile) => profile.kind === "trace" && profile.trace === board.traces[0]);
  const corner = profiles.find((profile) => profile.kind === "corner");
  assert.ok(straight && corner);

  const join = straight.centerline.at(-1);
  const previous = straight.centerline.at(-2);
  const startDistance = Math.hypot(
    corner.centerline[0].point.x - join.point.x,
    corner.centerline[0].point.y - join.point.y,
  );
  const cornerSamples = startDistance < 1e-8
    ? corner.centerline
    : [...corner.centerline].reverse();
  const incomingSlope = (join.width - previous.width) / Math.hypot(
    join.point.x - previous.point.x,
    join.point.y - previous.point.y,
  );
  const outgoingSlope = (cornerSamples[1].width - cornerSamples[0].width) / Math.hypot(
    cornerSamples[1].point.x - cornerSamples[0].point.x,
    cornerSamples[1].point.y - cornerSamples[0].point.y,
  );

  assert.ok(Math.abs(join.width - cornerSamples[0].width) < 1e-9);
  assert.ok(Math.abs(incomingSlope - outgoingSlope) < 0.05,
    `width slope jumped from ${incomingSlope} to ${outgoingSlope}`);
  assert.ok(corner.centerline.every((sample) => (
    sample.width >= DEFAULT_SETTINGS.neckdown_width - 1e-9
    && sample.width <= DEFAULT_SETTINGS.trace_width + 1e-9
  )));

  const reversedBoard = {
    ...board,
    traces: [
      { ...board.traces[1], start: board.traces[1].end, end: board.traces[1].start },
      { ...board.traces[0], start: board.traces[0].end, end: board.traces[0].start },
    ],
  };
  const reversedCorner = buildTraceProfiles(reversedBoard, { ...DEFAULT_SETTINGS, corner_radius: 3 })
    .find((profile) => profile.kind === "corner");
  assert.ok(reversedCorner);
  assert.deepEqual(
    reversedCorner.centerline.map((sample) => Number(sample.width.toFixed(10))),
    [...corner.centerline].reverse().map((sample) => Number(sample.width.toFixed(10))),
  );
});

test("rounds only safe degree-two corners and preserves branch topology", () => {
  const cornerBoard = {
    name: "corner",
    bounds: { min_x: 0, min_y: 0, max_x: 20, max_y: 20, width: 20, height: 20 },
    outline: [{ x: 0, y: 0 }, { x: 20, y: 0 }, { x: 20, y: 20 }, { x: 0, y: 20 }],
    traces: [
      { ...trace(), start: { x: 2, y: 10 }, end: { x: 10, y: 10 } },
      { ...trace(), start: { x: 10, y: 10 }, end: { x: 10, y: 18 } },
    ],
    pads: [], vias: [], warnings: [],
    stats: { traces: 2, pads: 0, vias: 0, holes: 0 },
  };
  const profiles = buildTraceProfiles(cornerBoard, DEFAULT_SETTINGS);
  const corner = profiles.find((profile) => profile.kind === "corner");
  assert.ok(corner);
  assert.ok(corner.centerline.some((sample) => sample.point.x < 10 && sample.point.y > 10));
  const center = { x: corner.centerline[0].point.x, y: corner.centerline.at(-1).point.y };
  const radius = Math.hypot(
    corner.centerline[0].point.x - center.x,
    corner.centerline[0].point.y - center.y,
  );
  assert.ok(corner.centerline.every((sample) => (
    Math.abs(Math.hypot(sample.point.x - center.x, sample.point.y - center.y) - radius) < 1e-9
  )));
  assert.ok(Math.abs(corner.length - radius * Math.PI / 2) < 1e-9);
  assert.ok(corner.centerline[0].tangent.x > 0.999999 && Math.abs(corner.centerline[0].tangent.y) < 1e-9);
  assert.ok(corner.centerline.at(-1).tangent.y > 0.999999 && Math.abs(corner.centerline.at(-1).tangent.x) < 1e-9);
  const cornerPolygon = traceProfilePolygons(corner)[0];
  const startSample = corner.centerline[0];
  const endSample = corner.centerline.at(-1);
  assert.ok(Math.hypot(
    cornerPolygon[0].x - startSample.point.x,
    cornerPolygon[0].y - (startSample.point.y + startSample.width / 2),
  ) < 1e-9);
  assert.ok(Math.hypot(
    cornerPolygon[corner.centerline.length - 1].x - (endSample.point.x - endSample.width / 2),
    cornerPolygon[corner.centerline.length - 1].y - endSample.point.y,
  ) < 1e-9);

  const padCornerBoard = {
    ...cornerBoard,
    traces: [
      { ...trace(0.6), start: { x: 2, y: 10 }, end: { x: 8, y: 10 } },
      { ...trace(0.6), start: { x: 8, y: 10 }, end: { x: 8, y: 18 } },
    ],
    pads: [{
      position: { x: 2, y: 10 }, size: { x: 4, y: 4 }, drill: 1.2,
      pad_type: "thru_hole", shape: "circle", rotation: 0,
      layers: ["*.Cu"], net_id: 1, net_name: "N1",
    }],
    stats: { traces: 2, pads: 1, vias: 0, holes: 1 },
  };
  const padCornerProfiles = buildTraceProfiles(padCornerBoard, DEFAULT_SETTINGS);
  const padStraight = padCornerProfiles[0];
  const padCorner = padCornerProfiles.find((profile) => profile.kind === "corner");
  assert.ok(padCorner);
  const straightJoin = padStraight.centerline.at(-1);
  const cornerJoin = [padCorner.centerline[0], padCorner.centerline.at(-1)]
    .sort((a, b) => (
      Math.hypot(a.point.x - straightJoin.point.x, a.point.y - straightJoin.point.y)
      - Math.hypot(b.point.x - straightJoin.point.x, b.point.y - straightJoin.point.y)
    ))[0];
  assert.ok(Math.abs(straightJoin.width - cornerJoin.width) < 1e-9);
  assert.ok(cornerJoin.width < padCorner.trunkWidth - 0.5, `corner join widened to ${cornerJoin.width}`);
  assert.ok(Math.max(...padStraight.lobes.flat().map((point) => point.x)) <= straightJoin.point.x + 1e-9);

  const roomyBoard = {
    ...cornerBoard,
    bounds: { min_x: 0, min_y: 0, max_x: 50, max_y: 50, width: 50, height: 50 },
    outline: [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 50 }, { x: 0, y: 50 }],
    traces: [
      { ...trace(), start: { x: 5, y: 25 }, end: { x: 25, y: 25 } },
      { ...trace(), start: { x: 25, y: 25 }, end: { x: 25, y: 45 } },
    ],
  };
  const roomyCorner = buildTraceProfiles(roomyBoard, { ...DEFAULT_SETTINGS, corner_radius: 4 })
    .find((profile) => profile.kind === "corner");
  assert.ok(roomyCorner);
  const roomyCenter = { x: roomyCorner.centerline[0].point.x, y: roomyCorner.centerline.at(-1).point.y };
  assert.ok(Math.abs(Math.hypot(
    roomyCorner.centerline[0].point.x - roomyCenter.x,
    roomyCorner.centerline[0].point.y - roomyCenter.y,
  ) - 4) < 1e-9);

  const reversed = {
    ...cornerBoard,
    traces: [cornerBoard.traces[0], { ...cornerBoard.traces[1], start: { x: 10, y: 18 }, end: { x: 10, y: 10 } }],
  };
  const reversedCorner = buildTraceProfiles(reversed, DEFAULT_SETTINGS)
    .find((profile) => profile.kind === "corner");
  assert.ok(reversedCorner);
  assert.ok(reversedCorner.centerline.every((sample) => (
    Math.abs(Math.hypot(sample.point.x - center.x, sample.point.y - center.y) - radius) < 1e-9
  )));

  const differentNet = {
    ...cornerBoard,
    traces: [cornerBoard.traces[0], { ...cornerBoard.traces[1], net_id: 2, net_name: "N2" }],
  };
  assert.equal(buildTraceProfiles(differentNet, DEFAULT_SETTINGS).some((profile) => profile.kind === "corner"), false);

  const protectedVia = {
    ...cornerBoard,
    vias: [{ position: { x: 10, y: 10 }, size: 2, drill: 0.8, layers: ["B.Cu"], net_id: 1, net_name: "N1" }],
  };
  assert.equal(buildTraceProfiles(protectedVia, DEFAULT_SETTINGS).some((profile) => profile.kind === "corner"), false);

  const branched = {
    ...cornerBoard,
    traces: [...cornerBoard.traces, { ...trace(), start: { x: 10, y: 10 }, end: { x: 18, y: 10 } }],
  };
  assert.equal(buildTraceProfiles(branched, DEFAULT_SETTINGS).some((profile) => profile.kind === "corner"), false);

  const doubledBack = {
    ...cornerBoard,
    traces: [cornerBoard.traces[0], { ...trace(), start: { x: 10, y: 10 }, end: { x: 2, y: 18 } }],
  };
  assert.equal(buildTraceProfiles(doubledBack, DEFAULT_SETTINGS).some((profile) => profile.kind === "corner"), false);
});
