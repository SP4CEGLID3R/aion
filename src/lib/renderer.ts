// Canvas drawing. Layout decisions live in layout.ts; this file only paints.

import { layoutMarkers, type MarkerLayout } from "./layout";
import { formatDistance, pointAt, screenAngle, type View } from "./projection";
import type { CubeView, RadarSettings, WorldPos } from "./types";

export interface Scene {
  settings: RadarSettings;
  player: WorldPos | null;
  headingDeg: number | null;
  cubes: readonly CubeView[];
  view: View;
}

const FONT = "600 11px system-ui, 'Segoe UI', sans-serif";

export function drawScene(ctx: CanvasRenderingContext2D, scene: Scene, width: number, height: number): void {
  const { settings, view } = scene;
  const c = settings.colors;
  ctx.clearRect(0, 0, width, height);

  // Disc.
  ctx.beginPath();
  ctx.arc(view.cx, view.cy, view.radiusPx, 0, Math.PI * 2);
  ctx.fillStyle = withAlpha(c.background, 0.78);
  ctx.fill();

  // Range rings with their distances.
  ctx.font = FONT;
  ctx.textAlign = "left";
  ctx.textBaseline = "middle";
  for (const f of [1 / 3, 2 / 3, 1]) {
    ctx.beginPath();
    ctx.arc(view.cx, view.cy, view.radiusPx * f, 0, Math.PI * 2);
    ctx.strokeStyle = withAlpha(c.ring, f === 1 ? 0.9 : 0.45);
    ctx.lineWidth = f === 1 ? 1.5 : 1;
    ctx.stroke();
    if (f < 1) {
      ctx.fillStyle = withAlpha(c.ring, 0.9);
      ctx.fillText(formatDistance(view.range * f), view.cx + 3, view.cy - view.radiusPx * f + 8);
    }
  }

  // Cross-hair along the view axes.
  ctx.beginPath();
  ctx.moveTo(view.cx - view.radiusPx, view.cy);
  ctx.lineTo(view.cx + view.radiusPx, view.cy);
  ctx.moveTo(view.cx, view.cy - view.radiusPx);
  ctx.lineTo(view.cx, view.cy + view.radiusPx);
  ctx.strokeStyle = withAlpha(c.ring, 0.25);
  ctx.lineWidth = 1;
  ctx.stroke();

  drawCompass(ctx, scene);

  const markers = layoutMarkers(scene.cubes, scene.player, view, settings);
  for (const m of markers) drawMarker(ctx, m, settings, width);

  if (scene.player) drawPlayer(ctx, scene);
}

function drawCompass(ctx: CanvasRenderingContext2D, scene: Scene): void {
  const { view, settings } = scene;
  ctx.font = "700 12px system-ui, 'Segoe UI', sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  for (const [label, deg] of [["N", 0], ["E", 90], ["S", 180], ["W", 270]] as const) {
    const p = pointAt(screenAngle(deg, view), view.radiusPx - 12, view);
    ctx.fillStyle = label === "N" ? "#ff6b6b" : withAlpha(settings.colors.label, 0.7);
    ctx.fillText(label, p.x, p.y);
  }
}

function drawPlayer(ctx: CanvasRenderingContext2D, scene: Scene): void {
  const { view, settings, headingDeg } = scene;
  ctx.save();
  ctx.translate(view.cx, view.cy);
  if (headingDeg !== null) {
    ctx.rotate(screenAngle(headingDeg, view));
    ctx.beginPath();
    ctx.moveTo(0, -9);
    ctx.lineTo(6.5, 7);
    ctx.lineTo(0, 3.5);
    ctx.lineTo(-6.5, 7);
    ctx.closePath();
  } else {
    ctx.beginPath();
    ctx.arc(0, 0, 5, 0, Math.PI * 2);
  }
  ctx.fillStyle = settings.colors.player;
  ctx.strokeStyle = "rgba(0,0,0,0.6)";
  ctx.lineWidth = 1.5;
  ctx.fill();
  ctx.stroke();
  ctx.restore();
}

function drawMarker(ctx: CanvasRenderingContext2D, m: MarkerLayout, settings: RadarSettings, width: number): void {
  const c = settings.colors;
  ctx.save();
  if (m.kind === "indicator") {
    ctx.translate(m.x, m.y);
    ctx.rotate(m.angle);
    ctx.beginPath();
    ctx.moveTo(0, -7);
    ctx.lineTo(6, 4);
    ctx.lineTo(-6, 4);
    ctx.closePath();
    ctx.fillStyle = c.indicator;
    ctx.strokeStyle = "rgba(0,0,0,0.6)";
    ctx.lineWidth = 1;
    ctx.fill();
    ctx.stroke();
    ctx.restore();
    if (m.text) {
      // Text sits inward of the arrow so it stays inside the disc.
      const back = 16;
      label(ctx, m.text, m.x - back * Math.sin(m.angle), m.y + back * Math.cos(m.angle), c.indicator, "center");
    }
    return;
  }

  // A diamond: solid for live, hollow and dashed for stale.
  const s = 6.5;
  ctx.translate(m.x, m.y);
  ctx.beginPath();
  ctx.moveTo(0, -s);
  ctx.lineTo(s, 0);
  ctx.lineTo(0, s);
  ctx.lineTo(-s, 0);
  ctx.closePath();
  if (m.kind === "live") {
    ctx.fillStyle = c.cube;
    ctx.fill();
    ctx.strokeStyle = "rgba(0,0,0,0.7)";
    ctx.lineWidth = 1.5;
    ctx.stroke();
  } else {
    ctx.setLineDash([2.5, 2]);
    ctx.strokeStyle = c.stale;
    ctx.lineWidth = 1.5;
    ctx.stroke();
  }
  ctx.restore();
  if (m.text) {
    const color = m.kind === "live" ? c.label : c.stale;
    ctx.font = FONT;
    // Flip the label to the left of the marker rather than clip it at the edge.
    const fits = m.x + 10 + ctx.measureText(m.text).width <= width - 2;
    label(ctx, m.text, fits ? m.x + 10 : m.x - 10, m.y, color, fits ? "left" : "right");
  }
}

function label(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  color: string,
  align: CanvasTextAlign,
): void {
  ctx.font = FONT;
  ctx.textAlign = align;
  ctx.textBaseline = "middle";
  ctx.lineWidth = 3;
  ctx.strokeStyle = "rgba(0,0,0,0.75)";
  ctx.strokeText(text, x, y);
  ctx.fillStyle = color;
  ctx.fillText(text, x, y);
}

/** `#rgb`/`#rrggbb` plus an alpha, as an rgba() string. */
export function withAlpha(hex: string, alpha: number): string {
  let h = hex.replace("#", "");
  if (h.length === 3) h = [...h].map((ch) => ch + ch).join("");
  const n = Number.parseInt(h.slice(0, 6), 16);
  if (Number.isNaN(n)) return `rgba(0,0,0,${alpha})`;
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

