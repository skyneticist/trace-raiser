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

  const reversed = {
    ...cornerBoard,
    traces: [cornerBoard.traces[0], { ...cornerBoard.traces[1], start: { x: 10, y: 18 }, end: { x: 10, y: 10 } }],
  };
  assert.equal(buildTraceProfiles(reversed, DEFAULT_SETTINGS).some((profile) => profile.kind === "corner"), true);

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
