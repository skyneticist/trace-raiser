import assert from "node:assert/strict";
import test from "node:test";

import {
  modelExportDisabledReason,
  summarizePrintabilityIssues,
} from "../app/lib/export-readiness.ts";

const issue = (severity, code = severity) => ({ severity, code, message: code });

test("warning-only boards remain eligible for model export", () => {
  const summary = summarizePrintabilityIssues([
    issue("warning", "BOTTOM_FOOTPRINT_TRANSFORM_APPROXIMATED"),
    issue("info", "metadata"),
  ]);

  assert.equal(summary.blockerCount, 0);
  assert.equal(summary.warningCount, 2);
  assert.equal(summary.firstBlocker, null);
  assert.equal(modelExportDisabledReason(true, false, summary.blockerCount), null);
});

test("blockers sort ahead of warnings and expose the export reason", () => {
  const summary = summarizePrintabilityIssues([
    issue("warning", "bottom-footprint"),
    issue("error", "UNFILLED_BCU_ZONE"),
    issue("warning", "edge-arc"),
  ]);

  assert.deepEqual(summary.orderedIssues.map(({ code }) => code), [
    "UNFILLED_BCU_ZONE",
    "bottom-footprint",
    "edge-arc",
  ]);
  assert.equal(summary.blockerCount, 1);
  assert.equal(summary.warningCount, 2);
  assert.match(modelExportDisabledReason(true, false, 1), /^1 printability blocker must/);
});

test("loading and processing states have distinct model-export reasons", () => {
  assert.equal(modelExportDisabledReason(false, false, 0), "Load a board to enable model export.");
  assert.equal(modelExportDisabledReason(true, true, 0), "Model processing is in progress.");
});
