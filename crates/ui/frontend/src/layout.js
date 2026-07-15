// Heuristic 3-column layout: sources left, filters middle, sinks right.
// Replace with a proper layout engine (elk.js, dagre) once we render
// edges between nodes.

const COL_X = { source: 60, filter: 460, sink: 860 };
const ROW_H = 56;

export function layoutNodes(flowNodes) {
  const counts = { source: 0, filter: 0, sink: 0 };
  return flowNodes.map((n) => {
    const k = n.sw?.kind === "unknown" ? "filter" : (n.sw?.kind ?? "filter");
    const row = counts[k]++;
    return { ...n, position: { x: COL_X[k], y: 40 + row * ROW_H } };
  });
}
