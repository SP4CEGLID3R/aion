// Canvas drawing. Layout decisions live in layout.ts; this file only paints.

import { formatMetres, nearestLive } from "./guide";
import { layoutMarkers, type MarkerLayout } from "./layout";
import { gridSegments, gridStep } from "./mapgrid";
import { DEG, formatDistance, pointAt, projectWorld, screenAngle, type View } from "./projection";
import type { CubeView, RadarSettings, WorldPos } from "./types";

/** The live game minimap, drawn under the radar (north-up, player-centred). */
export interface MinimapBackground {
  image: CanvasImageSource;
  pivotX: number;
  pivotY: number;
  /** Minimap pixels per metre. */
  pxPerM: number;
}

export interface Scene {
  settings: RadarSettings;
  player: WorldPos | null;
  headingDeg: number | null;
  cubes: readonly CubeView[];
  view: View;
  /** 1-sigma uncertainty of an estimated position, metres. */
  accuracyM?: number | null;
  /** Recent own positions, oldest first. */
  trail?: readonly WorldPos[];
  background?: MinimapBackground | null;
}

const FONT = "600 11px system-ui, 'Segoe UI', sans-serif";

export function drawScene(ctx: CanvasRenderingContext2D, scene: Scene, width: number, height: number): void {
  const { settings, view } = scene;
  const c = settings.colors;
  ctx.clearRect(0, 0, width, height);

  if (settings.style === "minimap") {
    drawMinimapScene(ctx, scene, width);
    return;
  }

  // Disc.
  ctx.beginPath();
  ctx.arc(view.cx, view.cy, view.radiusPx, 0, Math.PI * 2);
  ctx.fillStyle = withAlpha(c.background, 0.78);
  ctx.fill();
  if (scene.background && scene.player) drawBackground(ctx, scene, scene.background);

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

  if (settings.mapGrid && scene.player) drawGrid(ctx, scene);
  if (settings.showTrail) drawTrail(ctx, scene);

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

  if (scene.player) {
    drawAccuracy(ctx, scene);
    drawPlayer(ctx, scene);
    if (settings.guide.radarArrow) drawGuide(ctx, scene);
  }
}

/** The game's own minimap behind everything, at the radar's scale. */
function drawBackground(ctx: CanvasRenderingContext2D, scene: Scene, bg: MinimapBackground): void {
  const { view } = scene;
  if (!(bg.pxPerM > 0)) return;
  // Radar pixels per minimap pixel.
  const k = view.radiusPx / view.range / bg.pxPerM;
  ctx.save();
  ctx.beginPath();
  ctx.arc(view.cx, view.cy, view.radiusPx, 0, Math.PI * 2);
  ctx.clip();
  ctx.translate(view.cx, view.cy);
  // The minimap is north-up; turn it with the radar in heading-up mode.
  ctx.rotate(-view.upDeg * DEG);
  ctx.scale(k, k);
  ctx.globalAlpha = 0.8;
  ctx.imageSmoothingEnabled = true;
  ctx.drawImage(bg.image, -bg.pivotX, -bg.pivotY);
  ctx.restore();
}

/** An arrow on a ring around the player marker, towards the nearest cube. */
function drawGuide(ctx: CanvasRenderingContext2D, scene: Scene): void {
  const { view, settings } = scene;
  const target = nearestLive(scene.cubes);
  if (!target || target.bearingDeg === null || target.distance === null) return;
  const ring = 22;
  const angle = screenAngle(target.bearingDeg, view);
  const p = pointAt(angle, ring, view);
  ctx.save();
  ctx.beginPath();
  ctx.arc(view.cx, view.cy, ring, 0, Math.PI * 2);
  ctx.setLineDash([3, 4]);
  ctx.strokeStyle = withAlpha(settings.colors.cube, 0.45);
  ctx.lineWidth = 1.5;
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.translate(p.x, p.y);
  ctx.rotate(angle);
  ctx.beginPath();
  ctx.moveTo(0, -9);
  ctx.lineTo(7, 4);
  ctx.lineTo(0, 1);
  ctx.lineTo(-7, 4);
  ctx.closePath();
  ctx.fillStyle = settings.colors.cube;
  ctx.strokeStyle = "rgba(0,0,0,0.75)";
  ctx.lineWidth = 1.5;
  ctx.fill();
  ctx.stroke();
  ctx.restore();
  const tp = pointAt(angle, ring + 18, view);
  label(ctx, formatMetres(target.distance), tp.x, tp.y, settings.colors.cube, "center");
}

/** Markers only, on a clear background, for laying over the game's minimap. */
function drawMinimapScene(ctx: CanvasRenderingContext2D, scene: Scene, width: number): void {
  const { settings, view } = scene;
  // A faint outline so the window can be lined up with the minimap.
  ctx.beginPath();
  ctx.arc(view.cx, view.cy, view.radiusPx, 0, Math.PI * 2);
  ctx.strokeStyle = withAlpha(settings.colors.ring, 0.35);
  ctx.lineWidth = 1;
  ctx.stroke();
  const markers = layoutMarkers(scene.cubes, scene.player, view, settings);
  for (const m of markers) drawMarker(ctx, m, settings, width);
  if (scene.player) {
    drawAccuracy(ctx, scene);
    if (settings.guide.radarArrow) drawGuide(ctx, scene);
  }
}

/** World-fixed grid lines, clipped to the disc: they slide as you move. */
function drawGrid(ctx: CanvasRenderingContext2D, scene: Scene): void {
  const { view, settings, player } = scene;
  if (!player) return;
  ctx.save();
  ctx.beginPath();
  ctx.arc(view.cx, view.cy, view.radiusPx, 0, Math.PI * 2);
  ctx.clip();
  const step = gridStep(view.range);
  for (const major of [false, true]) {
    ctx.beginPath();
    for (const s of gridSegments(player, view, step)) {
      if (s.major !== major) continue;
      ctx.moveTo(s.x1, s.y1);
      ctx.lineTo(s.x2, s.y2);
    }
    ctx.strokeStyle = withAlpha(settings.colors.ring, major ? 0.45 : 0.25);
    ctx.lineWidth = 1;
    ctx.stroke();
  }
  ctx.restore();
}

/** Where you have been in the last minute, fading with age. */
function drawTrail(ctx: CanvasRenderingContext2D, scene: Scene): void {
  const { view, settings, player, trail } = scene;
  if (!player || !trail || trail.length < 2) return;
  ctx.save();
  ctx.beginPath();
  ctx.arc(view.cx, view.cy, view.radiusPx, 0, Math.PI * 2);
  ctx.clip();
  ctx.lineCap = "round";
  ctx.lineWidth = 2;
  for (let i = 1; i < trail.length; i++) {
    const a = projectWorld(player, trail[i - 1]!, view);
    const b = projectWorld(player, trail[i]!, view);
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.strokeStyle = withAlpha(settings.colors.player, 0.08 + 0.4 * (i / trail.length));
    ctx.stroke();
  }
  ctx.restore();
}

/** The estimate's uncertainty: you are probably inside this circle. */
function drawAccuracy(ctx: CanvasRenderingContext2D, scene: Scene): void {
  const { view, settings, accuracyM } = scene;
  if (!accuracyM || !Number.isFinite(accuracyM)) return;
  const r = Math.min(view.radiusPx, (accuracyM * view.radiusPx) / view.range);
  if (r < 4) return;
  ctx.save();
  ctx.beginPath();
  ctx.arc(view.cx, view.cy, r, 0, Math.PI * 2);
  ctx.fillStyle = withAlpha(settings.colors.player, 0.1);
  ctx.fill();
  ctx.setLineDash([4, 3]);
  ctx.strokeStyle = withAlpha(settings.colors.player, 0.55);
  ctx.lineWidth = 1;
  ctx.stroke();
  ctx.restore();
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

