// The Canvas backend's drawing pass — 05-viz.md §1.6, §5.3.
//
// Above 300 visible nodes SVG stops holding 60 fps, so the same [`Scene`] is
// painted into a 2D context instead. The function is kept free of React and of
// the DOM beyond the context itself, so it can be driven by a stub context in a
// unit test and asserted on as a *sequence of drawing calls* — which is the only
// way to check that the Canvas backend and the SVG backend agree.

import type { Scene, SceneNode } from "./scene";

/** The slice of `CanvasRenderingContext2D` this renderer uses. */
export type Ctx2D = Pick<
  CanvasRenderingContext2D,
  | "save"
  | "restore"
  | "beginPath"
  | "moveTo"
  | "lineTo"
  | "closePath"
  | "fill"
  | "stroke"
  | "fillRect"
  | "fillText"
  | "setLineDash"
  | "arc"
  | "translate"
  | "scale"
  | "clearRect"
> & {
  fillStyle: string | CanvasGradient | CanvasPattern;
  strokeStyle: string | CanvasGradient | CanvasPattern;
  lineWidth: number;
  globalAlpha: number;
  font: string;
  textAlign: CanvasTextAlign;
  textBaseline: CanvasTextBaseline;
};

export interface CanvasTheme {
  background: string;
  edge: string;
  impact: string;
  label: string;
  band: string;
  selection: string;
}

export const LIGHT_THEME: CanvasTheme = {
  background: "#ffffff",
  edge: "#8b93a1",
  impact: "#c07800",
  label: "#ffffff",
  band: "#f1f3f6",
  selection: "#111418",
};

function nodeOutline(ctx: Ctx2D, n: SceneNode): void {
  const { x, y, width: w, height: h } = n;
  ctx.beginPath();
  if (n.shape === "pill") {
    const r = h / 2;
    ctx.moveTo(x + r, y);
    ctx.lineTo(x + w - r, y);
    ctx.arc(x + w - r, y + r, r, -Math.PI / 2, Math.PI / 2);
    ctx.lineTo(x + r, y + h);
    ctx.arc(x + r, y + r, r, Math.PI / 2, (3 * Math.PI) / 2);
  } else if (n.clipped) {
    ctx.moveTo(x, y);
    ctx.lineTo(x + w - 8, y);
    ctx.lineTo(x + w, y + 8);
    ctx.lineTo(x + w, y + h);
    ctx.lineTo(x, y + h);
  } else {
    ctx.moveTo(x, y);
    ctx.lineTo(x + w, y);
    ctx.lineTo(x + w, y + h);
    ctx.lineTo(x, y + h);
  }
  ctx.closePath();
}

/**
 * Paint one frame. Order is bands → edges → nodes, matching the SVG document
 * order, so the two backends stack identically.
 */
export function paint(
  ctx: Ctx2D,
  scene: Scene,
  view: { zoom: number; panX: number; panY: number; width: number; height: number },
  theme: CanvasTheme = LIGHT_THEME,
): void {
  ctx.save();
  ctx.fillStyle = theme.background;
  ctx.fillRect(0, 0, view.width, view.height);
  ctx.translate(view.panX, view.panY);
  ctx.scale(view.zoom, view.zoom);

  for (const band of scene.bands) {
    ctx.fillStyle = theme.band;
    ctx.globalAlpha = 1;
    ctx.fillRect(band.x, -8, band.width, scene.height + 16);
  }

  for (const e of scene.edges) {
    if (e.points.length < 2) continue;
    ctx.globalAlpha = e.opacity;
    ctx.strokeStyle = e.impacted ? theme.impact : theme.edge;
    ctx.lineWidth = e.style.width;
    ctx.setLineDash(e.style.dash ? e.style.dash.split(" ").map(Number) : []);
    ctx.beginPath();
    ctx.moveTo(e.points[0].x, e.points[0].y);
    for (const p of e.points.slice(1)) ctx.lineTo(p.x, p.y);
    ctx.stroke();
    if (e.style.double) {
      ctx.beginPath();
      ctx.moveTo(e.points[0].x, e.points[0].y + 2.5);
      for (const p of e.points.slice(1)) ctx.lineTo(p.x, p.y + 2.5);
      ctx.stroke();
    }
    if (e.style.target_dot) {
      const end = e.points[e.points.length - 1];
      ctx.setLineDash([]);
      ctx.beginPath();
      ctx.arc(end.x, end.y, 3, 0, Math.PI * 2);
      ctx.stroke();
    }
  }
  ctx.setLineDash([]);

  ctx.font = "12px ui-sans-serif, system-ui, sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  for (const n of scene.nodes) {
    ctx.globalAlpha = n.opacity;
    ctx.fillStyle = n.colour;
    nodeOutline(ctx, n);
    ctx.fill();
    if (n.selected || n.impacted) {
      ctx.strokeStyle = n.selected ? theme.selection : theme.impact;
      ctx.lineWidth = n.selected ? 2.5 : 2;
      ctx.stroke();
    }
    ctx.fillStyle = theme.label;
    ctx.fillText(n.node.name, n.x + n.width / 2, n.y + n.height / 2);
    if (n.selfLag !== undefined) {
      ctx.strokeStyle = theme.edge;
      ctx.lineWidth = 1;
      ctx.setLineDash([6, 4]);
      ctx.beginPath();
      ctx.moveTo(n.x + n.width - 14, n.y);
      ctx.lineTo(n.x + n.width + 12, n.y + n.height / 2);
      ctx.lineTo(n.x + n.width - 14, n.y + n.height);
      ctx.stroke();
      ctx.setLineDash([]);
    }
  }
  ctx.globalAlpha = 1;
  ctx.restore();
}
