import type { GeneratorSettings } from "./pcb-core";

export const DEFAULT_SETTINGS: GeneratorSettings = {
  board_thickness: 1.6,
  trace_height: 0.5,
  trace_width: 2.5,
  width_mode: "auto",
  trace_style: "vintage",
  neckdown_width: 1.4,
  taper_length: 6,
  corner_radius: 5,
  teardrop_length: 3,
  teardrop_strength: 0.55,
  trace_clearance: 0.5,
  hole_compensation: 0.18,
};

const COMPATIBILITY_SETTINGS = {
  ...DEFAULT_SETTINGS,
  taper_length: 4,
  corner_radius: 3,
  teardrop_strength: 0.75,
};

export type SavedGeneratorSettings = Partial<GeneratorSettings> & {
  min_trace_width?: number;
  clearance?: number;
  side?: "F.Cu" | "B.Cu";
};

export function normalizeSettings(saved?: SavedGeneratorSettings): GeneratorSettings {
  const defaults = saved ? COMPATIBILITY_SETTINGS : DEFAULT_SETTINGS;
  const traceWidth = normalizedNumber(
    saved?.trace_width ?? saved?.min_trace_width,
    defaults.trace_width,
    1.2,
    4,
  );
  return {
    board_thickness: normalizedNumber(saved?.board_thickness, defaults.board_thickness, 0.8, 3),
    trace_height: normalizedNumber(saved?.trace_height, defaults.trace_height, 0.2, 1.4),
    trace_width: traceWidth,
    width_mode: saved?.width_mode === "preserve" ? "preserve" : "auto",
    // Saved projects created before trace styling existed retain their exact
    // straight/linear output. Fresh sessions use the new restrained preset.
    trace_style: saved?.trace_style === "soft" || saved?.trace_style === "vintage"
      ? saved.trace_style
      : saved ? "technical" : defaults.trace_style,
    neckdown_width: normalizedNumber(saved?.neckdown_width, defaults.neckdown_width, 0.8, traceWidth),
    taper_length: normalizedNumber(saved?.taper_length, defaults.taper_length, 0.5, 12),
    corner_radius: normalizedNumber(saved?.corner_radius, defaults.corner_radius, 0.5, 12),
    teardrop_length: normalizedNumber(saved?.teardrop_length, defaults.teardrop_length, 0.5, 10),
    teardrop_strength: normalizedNumber(saved?.teardrop_strength, defaults.teardrop_strength, 0, 1),
    trace_clearance: normalizedNumber(
      saved?.trace_clearance ?? saved?.clearance,
      defaults.trace_clearance,
      0,
      2,
    ),
    hole_compensation: normalizedNumber(saved?.hole_compensation, defaults.hole_compensation, 0, 0.6),
  };
}

function normalizedNumber(value: number | undefined, fallback: number, min: number, max: number) {
  const safe = typeof value === "number" && Number.isFinite(value) ? value : fallback;
  return Math.max(min, Math.min(max, safe));
}
