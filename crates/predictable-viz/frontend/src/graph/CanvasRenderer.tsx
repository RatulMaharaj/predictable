// The Canvas backend's React shell. All drawing lives in `canvas.ts`; this file
// owns only the element, the device-pixel-ratio and the hit test, and it mirrors
// the scene into an off-screen list so the graph stays keyboard- and
// screen-reader-navigable at 2,000 nodes (§5.2).

import { useEffect, useRef } from "react";
import { hitTest, type Scene } from "./scene";
import { paint } from "./canvas";

export function CanvasRenderer({
  scene,
  onSelect,
  onFocus,
}: {
  scene: Scene;
  onSelect: (id: string) => void;
  onFocus: (id: string) => void;
}) {
  const ref = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return; // jsdom without a canvas backend: the a11y list still renders
    const dpr = globalThis.devicePixelRatio ?? 1;
    const width = canvas.clientWidth || 900;
    const height = canvas.clientHeight || 600;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    const zoom = Math.min(
      1,
      Math.min(width / (scene.width + 24), height / (scene.height + 24)) || 1,
    );
    paint(ctx, scene, { zoom: zoom * dpr, panX: 12 * dpr, panY: 12 * dpr, width: canvas.width, height: canvas.height });
  }, [scene]);

  const pick = (event: React.MouseEvent<HTMLCanvasElement>) => {
    const canvas = ref.current;
    if (!canvas) return undefined;
    const box = canvas.getBoundingClientRect();
    const zoom = Math.min(
      1,
      Math.min(box.width / (scene.width + 24), box.height / (scene.height + 24)) || 1,
    );
    return hitTest(scene, (event.clientX - box.left - 12) / zoom, (event.clientY - box.top - 12) / zoom);
  };

  return (
    <div className="graph graph-canvas" data-testid="graph-canvas">
      <canvas
        ref={ref}
        onClick={(e) => {
          const hit = pick(e);
          if (hit) onSelect(hit.id);
        }}
        onDoubleClick={(e) => {
          const hit = pick(e);
          if (hit) onFocus(hit.id);
        }}
      />
      <ul className="a11y-nodes" aria-label="model dependency graph">
        {scene.nodes.map((n) => (
          <li key={n.id}>
            <button
              data-testid="graph-node"
              data-id={n.id}
              data-kind={n.node.kind}
              data-depth={n.node.depth}
              onClick={() => onSelect(n.id)}
              onDoubleClick={() => onFocus(n.id)}
            >
              {n.node.name}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
