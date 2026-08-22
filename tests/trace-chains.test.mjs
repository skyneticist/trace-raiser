import assert from "node:assert/strict";
import test from "node:test";
import { buildTraceChains } from "../app/lib/trace-chains.ts";

const trace = (x1, y1, x2, y2, width = 2.5) => ({
  start: { x: x1, y: y1 },
  end: { x: x2, y: y2 },
  width,
  layer: "B.Cu",
});

test("joins unordered adjacent segments into one continuous chain", () => {
  const chains = buildTraceChains([
    trace(10, 0, 20, 0),
    trace(0, 0, 10, 0),
    trace(20, 0, 30, 0),
  ], 2.5);

  assert.equal(chains.length, 1);
  assert.equal(chains[0].points.length, 4);
  assert.equal(chains[0].closed, false);
});

test("splits a branch into safe non-branching chains without losing edges", () => {
  const chains = buildTraceChains([
    trace(0, 0, 10, 0),
    trace(10, 0, 20, 0),
    trace(10, 0, 10, 10),
  ], 2.5);

  assert.equal(chains.length, 3);
  assert.equal(chains.reduce((edges, chain) => edges + chain.points.length - 1, 0), 3);
});

test("keeps cycles closed and separates width transitions", () => {
  const cycle = buildTraceChains([
    trace(0, 0, 10, 0),
    trace(10, 0, 10, 10),
    trace(10, 10, 0, 10),
    trace(0, 10, 0, 0),
  ], 2.5);
  assert.equal(cycle.length, 1);
  assert.equal(cycle[0].closed, true);

  const flooredWidths = buildTraceChains([
    trace(0, 0, 10, 0, 1.2),
    trace(10, 0, 20, 0, 2),
  ], 2.5);
  assert.equal(flooredWidths.length, 1);
  assert.equal(flooredWidths[0].width, 2.5);

  const widths = buildTraceChains([
    trace(0, 0, 10, 0, 2.5),
    trace(10, 0, 20, 0, 3.2),
  ], 2.5);
  assert.equal(widths.length, 2);
  assert.deepEqual(widths.map((chain) => chain.width).sort(), [2.5, 3.2]);
});
