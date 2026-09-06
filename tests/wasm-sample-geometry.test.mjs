import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { strFromU8, unzipSync } from "fflate";
import { stlTo3mf } from "../app/lib/three-mf.ts";

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
    let handle;
    if (method === "parse_kicad") {
      handle = core.parse_kicad(pointers[0], inputs[0].byteLength);
    } else if (method === "auto_layout") {
      handle = core.auto_layout(
        pointers[0], inputs[0].byteLength,
        pointers[1], inputs[1].byteLength,
      );
    } else {
      handle = core.generate_stl(
        pointers[0], inputs[0].byteLength,
        pointers[1], inputs[1].byteLength,
      );
    }
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

function parse3mf(bytes) {
  const archive = unzipSync(bytes);
  const model = strFromU8(archive["3D/3dmodel.model"]);
  const vertices = [...model.matchAll(/<vertex x="([^"]+)" y="([^"]+)" z="([^"]+)"\/>/g)]
    .map((match) => match.slice(1).map(Number));
  return [...model.matchAll(/<triangle v1="(\d+)" v2="(\d+)" v3="(\d+)"\/>/g)]
    .map((match) => match.slice(1).map((index) => vertices[Number(index)]));
}

function assertTwoFacesPerWeldedEdge(triangles, precision = 5) {
  const edgeCounts = new Map();
  for (const triangle of triangles) {
    const keys = triangle.map((vertex) => vertex.map((value) => value.toFixed(precision)).join(","));
    for (const [a, b] of [[keys[0], keys[1]], [keys[1], keys[2]], [keys[2], keys[0]]]) {
      const edge = a < b ? `${a}|${b}` : `${b}|${a}`;
      edgeCounts.set(edge, (edgeCounts.get(edge) ?? 0) + 1);
    }
  }
  for (const [edge, count] of edgeCounts) {
    assert.equal(count, 2, `two welded faces at edge ${edge}`);
  }
}

function pointInTriangle([px, py], [a, b, c]) {
  const sign = (u, v) => (px - v[0]) * (u[1] - v[1]) - (u[0] - v[0]) * (py - v[1]);
  const signs = [sign(a, b), sign(b, c), sign(c, a)];
  return !(signs.some((value) => value < -1e-5) && signs.some((value) => value > 1e-5));
}

test("shipped WASM emits a watertight sample board and routes the assessment board", async () => {
  const [wasmBytes, autoLayoutBytes, source, assessmentSource] = await Promise.all([
    readFile(new URL("../public/pcb_core.wasm", import.meta.url)),
    readFile(new URL("../public/autolayout_core.wasm", import.meta.url)),
    readFile(new URL("../public/sample-sensor.kicad_pcb", import.meta.url), "utf8"),
    readFile(new URL("../public/sample-autolayout-assessment.kicad_pcb", import.meta.url), "utf8"),
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
    taper_length: 6,
    corner_radius: 5,
    teardrop_length: 3,
    teardrop_strength: 0.55,
    trace_clearance: 0.5,
    hole_compensation: 0.18,
  }));
  const stl = callCore(core, "generate_stl", [
    encoder.encode(JSON.stringify(board)),
    settings,
  ]);
  const triangles = parseStl(stl);
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
  assertTwoFacesPerWeldedEdge(triangles);

  const threeMfTriangles = parse3mf(stlTo3mf(stl, "sample-sensor"));
  assert.equal(threeMfTriangles.length, triangles.length, "3MF preserves every STL triangle");
  assertTwoFacesPerWeldedEdge(threeMfTriangles);

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

  const autoLayoutModule = await WebAssembly.compile(autoLayoutBytes);
  const autoLayoutInstance = await WebAssembly.instantiate(autoLayoutModule, {});
  const autoLayoutCore = autoLayoutInstance.exports;
  const proposal = JSON.parse(decoder.decode(callCore(autoLayoutCore, "auto_layout", [
    encoder.encode(assessmentSource),
    encoder.encode(JSON.stringify({
      seed: 424243,
      placement_restarts: 8,
      placement_refinement_passes: 2,
      placement_max_grid_points: 100000,
      placement_grid_mm: 1,
      component_clearance_mm: 3,
      edge_clearance_mm: 1,
      routing_grid_mm: 0.5,
      trace_width_mm: 0.8,
      trace_clearance_mm: 0.5,
      routing_max_search_nodes: 250000,
      routing_reroute_passes: 6,
      bend_penalty_mm: 0.25,
      congestion_penalty_mm: 2,
    })),
  ])));
  assert.equal(proposal.schema_version, 1);
  assert.equal(proposal.requires_kicad_drc, true);
  assert.equal(proposal.metrics.component_count, 10);
  assert.equal(proposal.metrics.moved_component_count, 4);
  assert.equal(proposal.seed, 424243);
  assert.equal(proposal.metrics.routed_net_count, 12);
  assert.equal(proposal.metrics.total_net_count, 12);
  assert.ok(proposal.metrics.segment_count >= 20);
  assert.ok(proposal.metrics.bend_count >= 10);
  assert.ok(proposal.metrics.candidate_evaluations > 100_000);
  assert.equal(proposal.proposal_id, "sha256:430c6a6bdb2b8640027c0e249cf635b30f7d1d97ff53fdd63717d023b97dade6");
  assert.doesNotMatch(assessmentSource, /\(segment\b/);
  assert.match(proposal.candidate_source, /\(segment\b/);

  const candidate = JSON.parse(decoder.decode(callCore(core, "parse_kicad", [
    encoder.encode(proposal.candidate_source),
  ])));
  assert.equal(candidate.stats.traces, proposal.metrics.segment_count);
  assert.equal(candidate.stats.holes, 26);
});
