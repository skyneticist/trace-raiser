import assert from "node:assert/strict";
import test from "node:test";
import { normalizeSettings } from "../app/lib/settings.ts";

test("migrates legacy width and clearance keys into safe auto-neckdown defaults", () => {
  const settings = normalizeSettings({
    board_thickness: 1.8,
    trace_height: 0.45,
    min_trace_width: 2.8,
    clearance: 0.65,
    side: "F.Cu",
  });
  assert.deepEqual(settings, {
    board_thickness: 1.8,
    trace_height: 0.45,
    trace_width: 2.8,
    width_mode: "auto",
    trace_style: "technical",
    neckdown_width: 1.4,
    taper_length: 4,
    corner_radius: 3,
    teardrop_length: 3,
    teardrop_strength: 0.75,
    trace_clearance: 0.65,
    hole_compensation: 0.18,
  });
  assert.equal("side" in settings, false);
});

test("never returns undefined or out-of-range slider values", () => {
  const settings = normalizeSettings({ trace_width: 1.2, neckdown_width: 9, taper_length: Number.NaN });
  assert.equal(settings.neckdown_width, 1.2);
  assert.equal(settings.taper_length, 4);
  assert.ok(Object.values(settings).every((value) => value !== undefined));
});

test("uses vintage shaping for fresh work while preserving legacy project output", () => {
  assert.equal(normalizeSettings().trace_style, "vintage");
  assert.equal(normalizeSettings({}).trace_style, "technical");
  assert.equal(normalizeSettings({ trace_style: "soft" }).trace_style, "soft");
  assert.equal(normalizeSettings({ trace_style: "unknown" }).trace_style, "technical");
});
