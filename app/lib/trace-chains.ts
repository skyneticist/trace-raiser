import type { Point, Trace } from "./pcb-core";

export type TraceChain = {
  points: Point[];
  width: number;
  closed: boolean;
};

type GraphEdge = {
  startKey: string;
  endKey: string;
  width: number;
};

type GraphNode = {
  point: Point;
  edges: number[];
};

const ENDPOINT_PRECISION = 10_000;
const WIDTH_EPSILON = 1e-6;

/**
 * Turns unordered KiCad segments into maximal, non-branching paths. Branches
 * intentionally terminate a chain at their shared node so every edge is drawn
 * exactly once; cycles remain closed paths. Width changes also start a new
 * chain because Canvas uses one stroke width per path.
 */
export function buildTraceChains(traces: Trace[], traceWidthFloor: number): TraceChain[] {
  const nodes = new Map<string, GraphNode>();
  const edges: GraphEdge[] = [];

  const addNode = (point: Point, edgeIndex: number) => {
    const key = pointKey(point);
    const node = nodes.get(key);
    if (node) node.edges.push(edgeIndex);
    else nodes.set(key, { point, edges: [edgeIndex] });
    return key;
  };

  for (const trace of traces) {
    if (pointsEqual(trace.start, trace.end)) continue;
    const edgeIndex = edges.length;
    const startKey = addNode(trace.start, edgeIndex);
    const endKey = addNode(trace.end, edgeIndex);
    edges.push({
      startKey,
      endKey,
      width: Math.max(trace.width, traceWidthFloor),
    });
  }

  const visited = new Set<number>();
  const chains: TraceChain[] = [];

  const walk = (startKey: string, firstEdgeIndex: number) => {
    const firstEdge = edges[firstEdgeIndex];
    const width = firstEdge.width;
    const points: Point[] = [{ ...nodes.get(startKey)!.point }];
    let currentKey = startKey;
    let edgeIndex = firstEdgeIndex;
    let closed = false;

    while (!visited.has(edgeIndex)) {
      visited.add(edgeIndex);
      const edge = edges[edgeIndex];
      const nextKey = edge.startKey === currentKey ? edge.endKey : edge.startKey;
      points.push({ ...nodes.get(nextKey)!.point });

      if (nextKey === startKey && points.length > 2) {
        points.pop();
        closed = true;
        break;
      }

      const nextNode = nodes.get(nextKey)!;
      if (nextNode.edges.length !== 2) break;
      const nextEdgeIndex = nextNode.edges.find((candidate) => !visited.has(candidate));
      if (nextEdgeIndex === undefined) break;
      if (Math.abs(edges[nextEdgeIndex].width - width) > WIDTH_EPSILON) break;

      currentKey = nextKey;
      edgeIndex = nextEdgeIndex;
    }

    if (points.length > 1) chains.push({ points, width, closed });
  };

  // Endpoints and branch points define every maximal open chain.
  for (const [key, node] of nodes) {
    if (node.edges.length === 2) continue;
    for (const edgeIndex of node.edges) {
      if (!visited.has(edgeIndex)) walk(key, edgeIndex);
    }
  }

  // Anything left is a cycle, or a chain separated only by a width change.
  for (let edgeIndex = 0; edgeIndex < edges.length; edgeIndex += 1) {
    if (!visited.has(edgeIndex)) walk(edges[edgeIndex].startKey, edgeIndex);
  }

  return chains;
}

function pointKey(point: Point) {
  return `${Math.round(point.x * ENDPOINT_PRECISION)},${Math.round(point.y * ENDPOINT_PRECISION)}`;
}

function pointsEqual(a: Point, b: Point) {
  return pointKey(a) === pointKey(b);
}
