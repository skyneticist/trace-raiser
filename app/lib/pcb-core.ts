export type Point = { x: number; y: number };

export type Trace = {
  start: Point;
  end: Point;
  width: number;
  layer: string;
  net_id?: number | null;
  net_name?: string | null;
};

export type Pad = {
  position: Point;
  size: Point;
  drill: number | null;
  pad_type?: "smd" | "thru_hole" | "np_thru_hole" | string;
  shape: string;
  rotation: number;
  layers: string[];
  net_id?: number | null;
  net_name?: string | null;
  /** Compatibility aliases accepted in older saved project files. */
  pad_kind?: "smd" | "thru_hole" | "np_thru_hole" | string;
  kind?: "smd" | "thru_hole" | "np_thru_hole" | string;
};

export type Via = {
  position: Point;
  size: number;
  drill: number;
  layers: string[];
  net_id?: number | null;
  net_name?: string | null;
};

export type CopperZone = {
  layer: string;
  net_id?: number | null;
  net_name?: string | null;
  name?: string | null;
  kind: "copper" | "teardrop" | "keepout" | string;
  polygons: Point[][];
};

export type BoardWarning = {
  code: string;
  message: string;
  severity: "info" | "warning" | "error";
};

export type ParsedBoard = {
  name: string;
  bounds: {
    min_x: number;
    min_y: number;
    max_x: number;
    max_y: number;
    width: number;
    height: number;
  };
  outline: Point[];
  traces: Trace[];
  pads: Pad[];
  vias: Via[];
  /** Optional so editable projects saved before zone support remain valid. */
  zones?: CopperZone[];
  warnings: BoardWarning[];
  stats: {
    traces: number;
    pads: number;
    vias: number;
    holes: number;
  };
};

export type GeneratorSettings = {
  board_thickness: number;
  trace_height: number;
  trace_width: number;
  width_mode: "auto" | "preserve";
  trace_style: "technical" | "soft" | "vintage";
  neckdown_width: number;
  taper_length: number;
  corner_radius: number;
  teardrop_length: number;
  teardrop_strength: number;
  trace_clearance: number;
  hole_compensation: number;
};

type NettedFeature = { net_id?: number | null; net_name?: string | null };

function normalizeFeatureNet<T extends NettedFeature>(feature: T): T {
  const netId = feature.net_id;
  const normalizedId = typeof netId === "number" && Number.isInteger(netId) && netId > 0
    ? netId
    : null;
  return {
    ...feature,
    net_id: normalizedId,
    net_name: feature.net_name ?? null,
  };
}

/** Canonicalize legacy project data to the same contract emitted by Rust. */
export function normalizeParsedBoard(board: ParsedBoard): ParsedBoard {
  return {
    ...board,
    traces: board.traces.map(normalizeFeatureNet),
    pads: board.pads.map(normalizeFeatureNet),
    vias: board.vias.map(normalizeFeatureNet),
    zones: board.zones?.map(normalizeFeatureNet),
  };
}

type PcbCoreExports = {
  memory: WebAssembly.Memory;
  alloc: (length: number) => number;
  dealloc: (pointer: number, length: number) => void;
  parse_kicad: (pointer: number, length: number) => bigint;
  generate_stl: (
    boardPointer: number,
    boardLength: number,
    settingsPointer: number,
    settingsLength: number,
  ) => bigint;
  last_error: () => bigint;
};

let corePromise: Promise<PcbCoreExports> | null = null;

async function loadCore(): Promise<PcbCoreExports> {
  if (!corePromise) {
    const attempt = (async () => {
      const response = await fetch("/pcb_core.wasm");
      if (!response.ok) {
        throw new Error("The geometry engine could not be loaded.");
      }

      // Compiling first forces the instantiate(Module) overload, which returns
      // an Instance consistently. instantiate(bytes) returns a wrapper in
      // browsers, but some TypeScript runtimes type it as an Instance.
      const wasmModule = await WebAssembly.compile(await response.arrayBuffer());
      const instance = await WebAssembly.instantiate(wasmModule, {});
      return instance.exports as unknown as PcbCoreExports;
    })();
    corePromise = attempt.catch((error) => {
      // A transient fetch/compile failure must not poison the session. The
      // next parse or export preparation will make a fresh load attempt.
      corePromise = null;
      throw error;
    });
  }

  return corePromise;
}

function unpackResult(value: bigint) {
  return {
    pointer: Number(value >> 32n),
    length: Number(value & 0xffff_ffffn),
  };
}

function writeInput(core: PcbCoreExports, input: Uint8Array) {
  const pointer = core.alloc(input.byteLength);
  new Uint8Array(core.memory.buffer, pointer, input.byteLength).set(input);
  return pointer;
}

async function callCore(
  method: "parse_kicad" | "generate_stl",
  inputs: Uint8Array[],
): Promise<Uint8Array> {
  const core = await loadCore();
  const pointers = inputs.map((input) => writeInput(core, input));

  try {
    const packed =
      method === "parse_kicad"
        ? core.parse_kicad(pointers[0], inputs[0].byteLength)
        : core.generate_stl(
            pointers[0],
            inputs[0].byteLength,
            pointers[1],
            inputs[1].byteLength,
          );

    const { pointer, length } = unpackResult(packed);
    if (!pointer || !length) {
      const errorResult = core.last_error();
      const errorHandle = unpackResult(errorResult);
      if (errorHandle.pointer && errorHandle.length) {
        const message = new TextDecoder().decode(
          new Uint8Array(core.memory.buffer, errorHandle.pointer, errorHandle.length),
        );
        core.dealloc(errorHandle.pointer, errorHandle.length);
        throw new Error(message);
      }
      throw new Error("The geometry engine returned an empty result.");
    }

    const output = new Uint8Array(core.memory.buffer, pointer, length).slice();
    core.dealloc(pointer, length);
    return output;
  } finally {
    inputs.forEach((input, index) => {
      core.dealloc(pointers[index], input.byteLength);
    });
  }
}

export async function parseKicad(source: string): Promise<ParsedBoard> {
  const output = await callCore("parse_kicad", [new TextEncoder().encode(source)]);
  const decoded = new TextDecoder().decode(output);
  const result = JSON.parse(decoded) as ParsedBoard & { error?: string };

  if (result.error) {
    throw new Error(result.error);
  }

  return normalizeParsedBoard(result);
}

export async function generateStl(
  board: ParsedBoard,
  settings: GeneratorSettings,
): Promise<Uint8Array> {
  const encoder = new TextEncoder();
  const normalizedBoard = normalizeParsedBoard(board);
  return callCore("generate_stl", [
    encoder.encode(JSON.stringify(normalizedBoard)),
    encoder.encode(JSON.stringify(settings)),
  ]);
}
