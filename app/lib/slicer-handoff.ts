export const SLICER_PREFERENCE_KEY = "copperline-preferred-slicer";

export type SlicerPreference = "bambu" | "orca" | "prusa" | "cura" | "system";

export type SlicerOption = {
  value: SlicerPreference;
  label: string;
  handoffLabel: string;
};

export const SLICER_OPTIONS: readonly SlicerOption[] = [
  { value: "bambu", label: "Bambu Studio", handoffLabel: "Bambu Studio" },
  { value: "orca", label: "OrcaSlicer", handoffLabel: "OrcaSlicer" },
  { value: "prusa", label: "PrusaSlicer", handoffLabel: "PrusaSlicer" },
  { value: "cura", label: "Ultimaker Cura", handoffLabel: "Ultimaker Cura" },
  { value: "system", label: "System chooser", handoffLabel: "system chooser" },
] as const;

export function normalizeSlicerPreference(value: unknown): SlicerPreference {
  return SLICER_OPTIONS.some((option) => option.value === value)
    ? value as SlicerPreference
    : "bambu";
}

export function slicerOption(value: SlicerPreference): SlicerOption {
  return SLICER_OPTIONS.find((option) => option.value === value) ?? SLICER_OPTIONS[0];
}

export function decideSlicerHandoff(
  shareAvailable: boolean,
  canShareFile: boolean,
): "share" | "download" {
  return shareAvailable && canShareFile ? "share" : "download";
}

export function slicerActionLabel(value: SlicerPreference): string {
  const selected = slicerOption(value);
  return value === "system"
    ? "Share 3MF via system chooser"
    : `Share 3MF for ${selected.handoffLabel}`;
}

export function isCanceledShareError(error: unknown): boolean {
  return typeof error === "object"
    && error !== null
    && "name" in error
    && error.name === "AbortError";
}

export function slicerFallbackMessage(value: SlicerPreference): string {
  const destination = value === "system"
    ? "your preferred slicer"
    : slicerOption(value).handoffLabel;
  return `This browser cannot directly launch a desktop slicer. The 3MF was downloaded; open it manually in ${destination}.`;
}
