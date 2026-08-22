import assert from "node:assert/strict";
import test from "node:test";

import {
  decideSlicerHandoff,
  isCanceledShareError,
  normalizeSlicerPreference,
  slicerActionLabel,
  slicerFallbackMessage,
  slicerOption,
} from "../app/lib/slicer-handoff.ts";

test("normalizes device-local slicer preferences with Bambu Studio as the default", () => {
  assert.equal(normalizeSlicerPreference("orca"), "orca");
  assert.equal(normalizeSlicerPreference("prusa"), "prusa");
  assert.equal(normalizeSlicerPreference("cura"), "cura");
  assert.equal(normalizeSlicerPreference("system"), "system");
  assert.equal(normalizeSlicerPreference("unknown-slicer"), "bambu");
  assert.equal(normalizeSlicerPreference(null), "bambu");
  assert.equal(slicerOption(normalizeSlicerPreference(undefined)).label, "Bambu Studio");
  assert.equal(slicerActionLabel("bambu"), "Share 3MF for Bambu Studio");
  assert.equal(slicerActionLabel("system"), "Share 3MF via system chooser");
  assert.match(slicerFallbackMessage("cura"), /cannot directly launch a desktop slicer/i);
  assert.match(slicerFallbackMessage("cura"), /downloaded; open it manually in Ultimaker Cura/i);
});

test("shares only when Web Share and file sharing are both supported", () => {
  assert.equal(decideSlicerHandoff(true, true), "share");
  assert.equal(decideSlicerHandoff(false, true), "download");
  assert.equal(decideSlicerHandoff(true, false), "download");
  assert.equal(decideSlicerHandoff(false, false), "download");
  assert.equal(isCanceledShareError({ name: "AbortError" }), true);
  assert.equal(isCanceledShareError(new Error("share failed")), false);
});
