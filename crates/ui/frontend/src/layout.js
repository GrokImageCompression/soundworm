// Heuristic 3-column layout: sources left, filters middle, sinks right.
// A proper layout engine (elk.js, dagre) would route around edges; not
// worth it until the graph is dense enough to need it.

const COL_X = { source: 60, filter: 460, sink: 860 };
const ROW_H = 56;

// `saved` maps node name → {x, y} for positions the user has dragged.
// A saved node keeps its position; the rest fall back to the heuristic
// column stack.
export function layoutNodes(flowNodes, saved = {}) {
  const counts = { source: 0, filter: 0, sink: 0 };
  return flowNodes.map((n) => {
    const k = n.sw?.kind === "unknown" ? "filter" : (n.sw?.kind ?? "filter");
    const row = counts[k]++;
    const savedPos = saved[n.sw?.name];
    const position = savedPos ?? { x: COL_X[k], y: 40 + row * ROW_H };
    return { ...n, position };
  });
}
