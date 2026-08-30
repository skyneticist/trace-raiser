import type { BoardWarning } from "./pcb-core";

const SEVERITY_PRIORITY: Record<BoardWarning["severity"], number> = {
  error: 0,
  warning: 1,
  info: 2,
};

export function summarizePrintabilityIssues(issues: BoardWarning[]) {
  const blockerCount = issues.filter((issue) => issue.severity === "error").length;
  const warningCount = issues.length - blockerCount;
  const orderedIssues = [...issues].sort(
    (first, second) => SEVERITY_PRIORITY[first.severity] - SEVERITY_PRIORITY[second.severity],
  );

  return {
    blockerCount,
    warningCount,
    orderedIssues,
    firstBlocker: orderedIssues.find((issue) => issue.severity === "error") ?? null,
  };
}

export function modelExportDisabledReason(
  hasBoard: boolean,
  busy: boolean,
  blockerCount: number,
) {
  if (!hasBoard) return "Load a board to enable model export.";
  if (busy) return "Model processing is in progress.";
  if (blockerCount > 0) {
    return `${blockerCount} printability ${blockerCount === 1 ? "blocker must" : "blockers must"} be resolved before model export.`;
  }
  return null;
}
