import assert from "node:assert/strict";
import test from "node:test";
import {
  createPreviewProjection,
  RELIEF_Z_EXAGGERATION,
} from "../app/lib/preview-projection.ts";

const bounds = {
  min_x: 0,
  min_y: 0,
  max_x: 64,
  max_y: 39,
  width: 64,
  height: 39,
};

test("relief projection reserves space for the board and emphasized trace height", () => {
  const projection = createPreviewProjection({
    width: 740,
    height: 850,
    bounds,
    viewMode: "angled",
    boardThickness: 1.6,
    traceHeight: 0.5,
  });
  const center = { x: 32, y: 19.5 };
  const boardTop = projection.project(center, 0);
  const boardBottom = projection.project(center, -1.6);
  const copperTop = projection.project(center, projection.reliefTraceHeight);

  assert.equal(projection.reliefTraceHeight, 0.5 * RELIEF_Z_EXAGGERATION);
  assert.ok(copperTop.y < boardTop.y, "raised copper should project above the board surface");
  assert.ok(boardBottom.y > boardTop.y, "the board base should project below its surface");
  assert.ok(boardBottom.y - copperTop.y > 20, "the composite form should have a legible vertical profile");
});

test("top projection stays orthographic and ignores preview-only height", () => {
  const projection = createPreviewProjection({
    width: 740,
    height: 850,
    bounds,
    viewMode: "top",
    boardThickness: 1.6,
    traceHeight: 0.5,
  });
  const point = { x: 12, y: 8 };

  assert.equal(projection.angled, false);
  assert.equal(projection.reliefTraceHeight, 0);
  assert.deepEqual(projection.project(point, 0), projection.project(point, 12));
});
