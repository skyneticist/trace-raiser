import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function unpack(handle) {
  return {
    pointer: Number(handle >> 32n),
    length: Number(handle & 0xffff_ffffn),
  };
}

function callCore(core, method, inputs) {
  const pointers = inputs.map((input) => {
    const pointer = core.alloc(input.byteLength);
    new Uint8Array(core.memory.buffer, pointer, input.byteLength).set(input);
    return pointer;
  });

  try {
    const handle = method === "parse_kicad"
      ? core.parse_kicad(pointers[0], inputs[0].byteLength)
      : core.generate_stl(
          pointers[0], inputs[0].byteLength,
          pointers[1], inputs[1].byteLength,
        );
    const { pointer, length } = unpack(handle);
    assert.ok(pointer && length, `${method} returned an empty result`);
    const output = new Uint8Array(core.memory.buffer, pointer, length).slice();
    core.dealloc(pointer, length);
    return output;
  } finally {
    inputs.forEach((input, index) => core.dealloc(pointers[index], input.byteLength));
  }
}

function parseStl(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const count = view.getUint32(80, true);
  assert.equal(bytes.byteLength, 84 + count * 50, "binary STL length");
  return Array.from({ length: count }, (_, index) => {
    const offset = 84 + index * 50 + 12;
    return Array.from({ length: 3 }, (_, vertex) => [
      view.getFloat32(offset + vertex * 12, true),
      view.getFloat32(offset + vertex * 12 + 4, true),
      view.getFloat32(offset + vertex * 12 + 8, true),
    ]);
  });
}

function vertexKey(vertex) {
  return vertex.join(",");
}

function pointInTriangle([px, py], [a, b, c]) {
  const sign = (u, v) => (px - v[0]) * (u[1] - v[1]) - (u[0] - v[0]) * (py - v[1]);
  const signs = [sign(a, b), sign(b, c), sign(c, a)];
  return !(signs.some((value) => value < -1e-5) && signs.some((value) => value > 1e-5));
}

test("shipped WASM emits a watertight sample board with every drill open", async () => {
  const [wasmBytes, source] = await Promise.all([
    readFile(new URL("../public/pcb_core.wasm", import.meta.url)),
    readFile(new URL("../public/sample-sensor.kicad_pcb", import.meta.url), "utf8"),
  ]);
  const wasmModule = await WebAssembly.compile(wasmBytes);
  const instance = await WebAssembly.instantiate(wasmModule, {});
  const core = instance.exports;
  const board = JSON.parse(decoder.decode(callCore(core, "parse_kicad", [encoder.encode(source)])));

  assert.deepEqual(board.stats, { traces: 4, pads: 8, vias: 0, holes: 8 });
  assert.deepEqual(board.warnings, []);
  assert.equal(board.bounds.width, 64);
  assert.equal(board.bounds.height, 39);

  const settings = encoder.encode(JSON.stringify({
    board_thickness: 1.6,
    trace_height: 0.5,
    trace_width: 2.8,
    width_mode: "auto",
    trace_style: "vintage",
    neckdown_width: 1.4,
    taper_length: 4,
    corner_radius: 3,
    teardrop_length: 3,
    teardrop_strength: 0.75,
    trace_clearance: 0.5,
    hole_compensation: 0.18,
  }));
  const triangles = parseStl(callCore(core, "generate_stl", [
    encoder.encode(JSON.stringify(board)),
    settings,
  ]));
  const directedEdges = new Map();
  const undirectedEdges = new Map();
  const faces = new Set();
  const zLevels = new Set();
  let signedVolume = 0;

  for (const triangle of triangles) {
    assert.ok(triangle.flat().every(Number.isFinite), "finite vertices");
    triangle.forEach((vertex) => zLevels.add(Number(vertex[2].toFixed(5))));
    const [a, b, c] = triangle;
    const ab = b.map((value, index) => value - a[index]);
    const ac = c.map((value, index) => value - a[index]);
    const cross = [
      ab[1] * ac[2] - ab[2] * ac[1],
      ab[2] * ac[0] - ab[0] * ac[2],
      ab[0] * ac[1] - ab[1] * ac[0],
    ];
    assert.ok(cross.reduce((sum, value) => sum + value * value, 0) > 1e-12, "nonzero face area");

    const keys = triangle.map(vertexKey);
    const face = [...keys].sort().join("|");
    assert.ok(!faces.has(face), "no duplicate faces");
    faces.add(face);
    for (const [from, to] of [[keys[0], keys[1]], [keys[1], keys[2]], [keys[2], keys[0]]]) {
      directedEdges.set(`${from}>${to}`, (directedEdges.get(`${from}>${to}`) ?? 0) + 1);
      const edge = from < to ? `${from}|${to}` : `${to}|${from}`;
      undirectedEdges.set(edge, (undirectedEdges.get(edge) ?? 0) + 1);
    }
    signedVolume += (
      a[0] * (b[1] * c[2] - b[2] * c[1])
      - a[1] * (b[0] * c[2] - b[2] * c[0])
      + a[2] * (b[0] * c[1] - b[1] * c[0])
    ) / 6;
  }

  for (const [edge, count] of undirectedEdges) {
    assert.equal(count, 2, `two faces at edge ${edge}`);
    const [a, b] = edge.split("|");
    assert.equal(directedEdges.get(`${a}>${b}`), 1, "forward edge occurrence");
    assert.equal(directedEdges.get(`${b}>${a}`), 1, "reverse edge occurrence");
  }
  assert.ok(signedVolume > 0, "positive signed volume");
  assert.deepEqual([...zLevels].sort((a, b) => a - b), [0, 1.6, 2.1]);

  const drillAxes = board.pads.filter((pad) => pad.drill).map((pad) => [
    board.bounds.max_x - pad.position.x,
    pad.position.y - board.bounds.min_y,
  ]);
  assert.equal(drillAxes.length, 8);
  for (const center of drillAxes) {
    assert.ok(!triangles.some((triangle) => (
      Math.abs(triangle[0][2] - triangle[1][2]) < 1e-5
      && Math.abs(triangle[1][2] - triangle[2][2]) < 1e-5
      && pointInTriangle(center, triangle)
    )), `drill axis ${center.join(",")} is open`);
  }
});
