import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { buildTraceProfiles, padPolygon, traceProfilePolygons } from "../app/lib/manufacturing-geometry.ts";
import { DEFAULT_SETTINGS } from "../app/lib/settings.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function unpack(handle) {
  return { pointer: Number(handle >> 32n), length: Number(handle & 0xffff_ffffn) };
}

function generate(core, board, settings) {
  const inputs = [encoder.encode(JSON.stringify(board)), encoder.encode(JSON.stringify(settings))];
  const pointers = inputs.map((input) => {
    const pointer = core.alloc(input.byteLength);
    new Uint8Array(core.memory.buffer, pointer, input.byteLength).set(input);
    return pointer;
  });
  try {
    const handle = core.generate_stl(pointers[0], inputs[0].byteLength, pointers[1], inputs[1].byteLength);
    const { pointer, length } = unpack(handle);
    if (!pointer || !length) {
      const error = unpack(core.last_error());
      const message = error.pointer
        ? decoder.decode(new Uint8Array(core.memory.buffer, error.pointer, error.length))
        : "empty WASM result";
      if (error.pointer) core.dealloc(error.pointer, error.length);
      assert.fail(message);
    }
    const output = new Uint8Array(core.memory.buffer, pointer, length).slice();
    core.dealloc(pointer, length);
    return output;
  } finally {
    inputs.forEach((input, index) => core.dealloc(pointers[index], input.byteLength));
  }
}

function copperTop(stl, z) {
  const view = new DataView(stl.buffer, stl.byteOffset, stl.byteLength);
  const count = view.getUint32(80, true);
  let area = 0;
  const vertices = [];
  for (let index = 0; index < count; index += 1) {
    const offset = 84 + index * 50 + 12;
    const triangle = Array.from({ length: 3 }, (_, vertex) => [
      view.getFloat32(offset + vertex * 12, true),
      view.getFloat32(offset + vertex * 12 + 4, true),
      view.getFloat32(offset + vertex * 12 + 8, true),
    ]);
    if (!triangle.every((point) => Math.abs(point[2] - z) < 1e-5)) continue;
    vertices.push(...triangle);
    const [a, b, c] = triangle;
    area += Math.abs((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2;
  }
  return { area, bounds: pointBounds(vertices.map(([x, y]) => ({ x, y }))) };
}

function previewCopper(board, settings) {
  const sourcePolygons = [
    ...buildTraceProfiles(board, settings).flatMap((profile) => traceProfilePolygons(profile)),
    ...board.pads.map(padPolygon),
    ...(board.zones ?? [])
      .filter((zone) => zone.layer === "B.Cu" && zone.kind !== "keepout"
        && !(zone.kind === "teardrop" && settings.width_mode === "auto" && settings.trace_style === "vintage"))
      .flatMap((zone) => zone.polygons),
  ];
  const polygons = sourcePolygons.map((polygon) => polygon.map((point) => ({
    x: board.bounds.max_x - point.x,
    y: point.y - board.bounds.min_y,
  })));
  const drills = board.pads.filter((pad) => pad.drill).map((pad) => ({
    x: board.bounds.max_x - pad.position.x,
    y: pad.position.y - board.bounds.min_y,
    radius: (pad.drill + settings.hole_compensation) / 2,
  }));
  const step = 0.2;
  let occupied = 0;
  for (let y = step / 2; y < board.bounds.height; y += step) {
    for (let x = step / 2; x < board.bounds.width; x += step) {
      const point = { x, y };
      const copper = polygons.some((polygon) => pointInPolygon(point, polygon));
      const drilled = drills.some((drill) => Math.hypot(x - drill.x, y - drill.y) < drill.radius);
      if (copper && !drilled) occupied += 1;
    }
  }
  return {
    area: occupied * step * step,
    bounds: pointBounds(polygons.flat()),
  };
}

function pointBounds(points) {
  return points.reduce((bounds, point) => ({
    minX: Math.min(bounds.minX, point.x), minY: Math.min(bounds.minY, point.y),
    maxX: Math.max(bounds.maxX, point.x), maxY: Math.max(bounds.maxY, point.y),
  }), { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity });
}

function pointInPolygon(point, polygon) {
  let inside = false;
  for (let index = 0, previous = polygon.length - 1; index < polygon.length; previous = index, index += 1) {
    const a = polygon[previous];
    const b = polygon[index];
    if ((a.y > point.y) !== (b.y > point.y)
      && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x) inside = !inside;
  }
  return inside;
}

test("TypeScript preview and shipped Rust/WASM agree on styled copper extents and area", async () => {
  const wasm = await WebAssembly.compile(await readFile(new URL("../public/pcb_core.wasm", import.meta.url)));
  const core = (await WebAssembly.instantiate(wasm, {})).exports;
  const pad = (position, size, shape, rotation) => ({
    position, size, drill: 1.05, pad_type: "thru_hole", shape, rotation,
    layers: ["*.Cu"], net_id: 1, net_name: "FLOW",
  });
  const trace = (start, end, width) => ({
    start, end, width, layer: "B.Cu", net_id: 1, net_name: "FLOW",
  });
  const board = {
    name: "parity",
    bounds: { min_x: 0, min_y: 0, max_x: 50, max_y: 50, width: 50, height: 50 },
    outline: [{ x: 0, y: 0 }, { x: 50, y: 0 }, { x: 50, y: 50 }, { x: 0, y: 50 }],
    traces: [
      trace({ x: 10, y: 12 }, { x: 22, y: 12 }, 0.6),
      trace({ x: 10, y: 25 }, { x: 25, y: 25 }, 0.6),
      trace({ x: 25, y: 25 }, { x: 35, y: 35 }, 3),
      trace({ x: 35, y: 35 }, { x: 42, y: 35 }, 0.6),
    ],
    pads: [
      pad({ x: 10, y: 12 }, { x: 4, y: 4 }, "circle", 0),
      pad({ x: 22, y: 12 }, { x: 4, y: 4 }, "circle", 0),
      pad({ x: 10, y: 25 }, { x: 4, y: 4 }, "circle", 0),
      pad({ x: 42, y: 35 }, { x: 5, y: 3 }, "oval", 30),
    ],
    zones: [{
      layer: "B.Cu", kind: "copper", net_id: 3, net_name: "PLANE",
      polygons: [[{ x: 2, y: 2 }, { x: 8, y: 2 }, { x: 8, y: 8 }, { x: 2, y: 8 }]],
    }],
    vias: [], warnings: [],
    stats: { traces: 4, pads: 4, vias: 0, holes: 4 },
  };

  for (const trace_style of ["technical", "soft", "vintage"]) {
    const settings = { ...DEFAULT_SETTINGS, trace_style };
    const preview = previewCopper(board, settings);
    const exported = copperTop(generate(core, board, settings), settings.board_thickness + settings.trace_height);
    for (const key of ["minX", "minY", "maxX", "maxY"]) {
      assert.ok(Math.abs(preview.bounds[key] - exported.bounds[key]) < 0.08,
        `${trace_style} ${key}: preview ${preview.bounds[key]}, export ${exported.bounds[key]}`);
    }
    assert.ok(Math.abs(preview.area - exported.area) / exported.area < 0.04,
      `${trace_style} area: preview ${preview.area}, export ${exported.area}`);
  }
});
