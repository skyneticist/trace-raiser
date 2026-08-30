export type AutoLayoutOptions = {
  seed: number;
  placement_restarts: number;
  placement_refinement_passes: number;
  placement_max_grid_points: number;
  placement_grid_mm: number;
  component_clearance_mm: number;
  edge_clearance_mm: number;
  routing_grid_mm: number;
  trace_width_mm: number;
  trace_clearance_mm: number;
  routing_max_search_nodes: number;
  routing_reroute_passes: number;
  bend_penalty_mm: number;
  congestion_penalty_mm: number;
};

export type AutoLayoutMetrics = {
  component_count: number;
  moved_component_count: number;
  routed_net_count: number;
  total_net_count: number;
  segment_count: number;
  trace_length_mm: number;
  bend_count: number;
  candidate_evaluations: number;
};

export type AutoLayoutResult = {
  schema_version: 1;
  proposal_id: string;
  candidate_source: string;
  requires_kicad_drc: true;
  seed: number;
  metrics: AutoLayoutMetrics;
};

export const BALANCED_AUTO_LAYOUT_OPTIONS: AutoLayoutOptions = {
  seed: 424_242,
  placement_restarts: 8,
  placement_refinement_passes: 2,
  placement_max_grid_points: 100_000,
  placement_grid_mm: 1,
  component_clearance_mm: 1,
  edge_clearance_mm: 1,
  routing_grid_mm: 0.5,
  trace_width_mm: 1,
  trace_clearance_mm: 0.8,
  routing_max_search_nodes: 250_000,
  routing_reroute_passes: 6,
  bend_penalty_mm: 0.25,
  congestion_penalty_mm: 2,
};

type AutoLayoutCoreExports = {
  memory: WebAssembly.Memory;
  alloc: (length: number) => number;
  dealloc: (pointer: number, length: number) => void;
  auto_layout: (
    sourcePointer: number,
    sourceLength: number,
    optionsPointer: number,
    optionsLength: number,
  ) => bigint;
  last_error: () => bigint;
};

let corePromise: Promise<AutoLayoutCoreExports> | null = null;

async function loadCore(): Promise<AutoLayoutCoreExports> {
  if (!corePromise) {
    corePromise = (async () => {
      const response = await fetch("/autolayout_core.wasm");
      if (!response.ok) throw new Error("The AutoLayout engine could not be loaded.");
      const wasmModule = await WebAssembly.compile(await response.arrayBuffer());
      const instance = await WebAssembly.instantiate(wasmModule, {});
      return instance.exports as unknown as AutoLayoutCoreExports;
    })();
  }
  return corePromise;
}

function unpackResult(value: bigint) {
  return {
    pointer: Number(value >> 32n),
    length: Number(value & 0xffff_ffffn),
  };
}

function writeInput(core: AutoLayoutCoreExports, input: Uint8Array) {
  const pointer = core.alloc(input.byteLength);
  new Uint8Array(core.memory.buffer, pointer, input.byteLength).set(input);
  return pointer;
}

export async function generateAutoLayout(
  source: string,
  options: AutoLayoutOptions,
): Promise<AutoLayoutResult> {
  const core = await loadCore();
  const encoder = new TextEncoder();
  const inputs = [encoder.encode(source), encoder.encode(JSON.stringify(options))];
  const pointers = inputs.map((input) => writeInput(core, input));
  try {
    const packed = core.auto_layout(
      pointers[0],
      inputs[0].byteLength,
      pointers[1],
      inputs[1].byteLength,
    );
    const { pointer, length } = unpackResult(packed);
    if (!pointer || !length) {
      const errorHandle = unpackResult(core.last_error());
      if (errorHandle.pointer && errorHandle.length) {
        const message = new TextDecoder().decode(
          new Uint8Array(core.memory.buffer, errorHandle.pointer, errorHandle.length),
        );
        core.dealloc(errorHandle.pointer, errorHandle.length);
        throw new Error(message);
      }
      throw new Error("The AutoLayout engine returned an empty result.");
    }
    const decoded = new TextDecoder().decode(
      new Uint8Array(core.memory.buffer, pointer, length),
    );
    core.dealloc(pointer, length);
    const result = JSON.parse(decoded) as AutoLayoutResult;
    if (result.schema_version !== 1 || !result.candidate_source || !result.requires_kicad_drc) {
      throw new Error("The AutoLayout engine returned an unsupported proposal.");
    }
    return result;
  } finally {
    inputs.forEach((input, index) => core.dealloc(pointers[index], input.byteLength));
  }
}
