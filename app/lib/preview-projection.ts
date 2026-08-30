import type { ParsedBoard, Point } from "./pcb-core";

export type PreviewViewMode = "angled" | "top";

export const RELIEF_Z_EXAGGERATION = 3;

const RELIEF_Y_SHEAR = 0.38;
const RELIEF_X_RISE = 0.16;
const RELIEF_Y_SCALE = 0.46;

export function createPreviewProjection({
  width,
  height,
  bounds,
  viewMode,
  boardThickness,
  traceHeight,
}: {
  width: number;
  height: number;
  bounds: ParsedBoard["bounds"];
  viewMode: PreviewViewMode;
  boardThickness: number;
  traceHeight: number;
}) {
  const padX = Math.max(28, Math.min(52, width * 0.07));
  const padY = Math.max(28, Math.min(52, height * 0.07));
  const availableWidth = Math.max(80, width - padX * 2);
  const availableHeight = Math.max(80, height - padY * 2);
  const angled = viewMode === "angled";
  const reliefTraceHeight = angled ? traceHeight * RELIEF_Z_EXAGGERATION : 0;
  const projectedWidth = angled
    ? bounds.width + bounds.height * RELIEF_Y_SHEAR
    : bounds.width;
  const projectedHeight = angled
    ? bounds.height * RELIEF_Y_SCALE
      + bounds.width * RELIEF_X_RISE
      + boardThickness
      + reliefTraceHeight
    : bounds.height;
  const scale = Math.min(availableWidth / projectedWidth, availableHeight / projectedHeight);
  const centerX = width / 2;
  const centerY = angled
    ? height / 2 + (reliefTraceHeight - boardThickness) * scale / 2
    : height / 2;

  const project = (point: Point, z = 0) => {
    const x = point.x - bounds.min_x - bounds.width / 2;
    const y = point.y - bounds.min_y - bounds.height / 2;
    const mirroredX = -x;
    return angled
      ? {
          x: centerX + (mirroredX - y * RELIEF_Y_SHEAR) * scale,
          y: centerY + (mirroredX * RELIEF_X_RISE + y * RELIEF_Y_SCALE) * scale - z * scale,
        }
      : { x: centerX + mirroredX * scale, y: centerY + y * scale };
  };

  return { angled, project, reliefTraceHeight, scale };
}
