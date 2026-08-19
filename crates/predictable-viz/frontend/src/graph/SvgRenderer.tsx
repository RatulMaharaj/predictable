// The SVG backend — 05-viz.md §1.6.
//
// Below `CANVAS_THRESHOLD` visible nodes, SVG wins: hit-testing, CSS, focus rings
// and screen-reader text all come for free, and every node is a real element a
// Playwright golden-image test and a keyboard user can both find.

import type { Scene, SceneNode } from "./scene";

function nodePath(n: SceneNode): string {
  const { x, y, width: w, height: h } = n;
  const clip = n.clipped ? 8 : 0;
  if (n.shape === "pill") {
    const r = h / 2;
    return `M${x + r},${y} H${x + w - r} A${r},${r} 0 0 1 ${x + w},${y + h / 2} A${r},${r} 0 0 1 ${x + w - r},${y + h} H${x + r} A${r},${r} 0 0 1 ${x},${y + h / 2} A${r},${r} 0 0 1 ${x + r},${y} Z`;
  }
  if (n.shape === "table") {
    const r = 9;
    return `M${x + r},${y} H${x + w - r} Q${x + w},${y} ${x + w},${y + r} V${y + h - r} Q${x + w},${y + h} ${x + w - r},${y + h} H${x + r} Q${x},${y + h} ${x},${y + h - r} V${y + r} Q${x},${y} ${x + r},${y} Z`;
  }
  if (clip) {
    return `M${x},${y} H${x + w - clip} L${x + w},${y + clip} V${y + h} H${x} Z`;
  }
  return `M${x},${y} H${x + w} V${y + h} H${x} Z`;
}

function polyline(points: { x: number; y: number }[]): string {
  return points.map((p, i) => `${i === 0 ? "M" : "L"}${p.x},${p.y}`).join(" ");
}

export function SvgRenderer({
  scene,
  onSelect,
  onFocus,
}: {
  scene: Scene;
  onSelect: (id: string) => void;
  onFocus: (id: string) => void;
}) {
  return (
    <svg
      className="graph graph-svg"
      data-testid="graph-svg"
      viewBox={`-12 -12 ${scene.width + 24} ${scene.height + 24}`}
      role="group"
      aria-label="model dependency graph"
    >
      <g className="bands">
        {scene.bands.map((band) => (
          <g key={band.depth}>
            <rect
              className="band"
              x={band.x}
              y={-8}
              width={band.width}
              height={scene.height + 16}
              data-depth={band.depth}
            />
            <text className="band-label" x={band.x + 4} y={-1}>
              {band.depth}
            </text>
          </g>
        ))}
      </g>

      <g className="edges">
        {scene.edges.map((e, i) => {
          const points = e.points;
          if (points.length < 2) return null;
          const mid = points[Math.floor(points.length / 2)];
          const end = points[points.length - 1];
          return (
            <g
              key={`${e.edge.from}->${e.edge.to}#${i}`}
              className={`edge edge-${e.style.kind}${e.impacted ? " impacted" : ""}`}
              opacity={e.opacity}
              data-testid="graph-edge"
              data-from={e.edge.from}
              data-to={e.edge.to}
              data-lag={e.edge.lag ?? "table"}
            >
              <path d={polyline(points)} strokeDasharray={e.style.dash} strokeWidth={e.style.width} />
              {e.style.double && (
                <path
                  className="edge-double"
                  d={polyline(points.map((p) => ({ x: p.x, y: p.y + 2.5 })))}
                  strokeWidth={e.style.width}
                />
              )}
              {e.style.target_dot && <circle className="edge-dot" cx={end.x} cy={end.y} r={3} />}
              {e.style.label && (
                <text className="edge-label" x={mid.x} y={mid.y - 4}>
                  {e.style.label}
                </text>
              )}
            </g>
          );
        })}
      </g>

      <g className="nodes">
        {scene.nodes.map((n) => (
          <g
            key={n.id}
            className={`node node-${n.shape}${n.selected ? " selected" : ""}${n.impacted ? " impacted" : ""}`}
            opacity={n.opacity}
            data-testid="graph-node"
            data-id={n.id}
            data-kind={n.node.kind}
            data-stage={n.node.stage}
            data-depth={n.node.depth}
            tabIndex={0}
            role="button"
            aria-label={`${n.node.name}, ${n.node.kind}, ${n.node.shape}, unit ${n.node.unit}, depth ${n.node.depth}`}
            onClick={() => onSelect(n.id)}
            onDoubleClick={() => onFocus(n.id)}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onSelect(n.id);
              }
              if (event.key === "f") onFocus(n.id);
            }}
          >
            <path className="node-body" d={nodePath(n)} style={{ fill: n.colour }} />
            <text className="node-label" x={n.x + n.width / 2} y={n.y + n.height / 2 + 4}>
              {n.node.name}
            </text>
            {n.selfLag !== undefined && (
              <g className="self-loop" data-testid="self-loop" data-id={n.id}>
                <path
                  d={`M${n.x + n.width - 14},${n.y} C${n.x + n.width + 16},${n.y - 22} ${n.x + n.width + 16},${n.y + n.height + 22} ${n.x + n.width - 14},${n.y + n.height}`}
                  strokeDasharray="6 4"
                />
                <text x={n.x + n.width + 20} y={n.y + n.height / 2 + 3}>
                  t-{n.selfLag}
                </text>
              </g>
            )}
            {n.lint && (
              <g className="lint-badge" data-testid="lint-badge" data-code={n.lint.code}>
                <path d={`M${n.x + 4},${n.y + 11} L${n.x + 10},${n.y + 1} L${n.x + 16},${n.y + 11} Z`} />
                <title>
                  {n.lint.code}: {n.lint.severity}
                </title>
              </g>
            )}
          </g>
        ))}
      </g>
    </svg>
  );
}
