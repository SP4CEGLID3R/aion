// The arrow around the character, drawn over the game: a ring on the ground
// with an arrow towards the nearest live cube, turned with the camera (from
// the minimap's view cone) so "ahead" is where you look. A label says how
// far and whether it is up or down; on top of the cube the ring turns into a
// green "cube here" circle.

import { backend } from "./lib/bridge";
import {
  formatMetres,
  HEIGHT_HINT_M,
  heightCue,
  heightText,
  isHere,
  nearestLive,
  relativeBearing,
  ringPoint,
  type HeightCue,
} from "./lib/guide";
import { withAlpha } from "./lib/renderer";
import { DEFAULT_SETTINGS, type RadarFrame, type RadarSettings } from "./lib/types";

const canvas = document.querySelector<HTMLCanvasElement>("#guide")!;
const ctx = canvas.getContext("2d")!;

/** The window is this many ring widths wide (GUIDE_AREA in src-tauri). */
const AREA_W = 2.2;
const HERE_COLOR = "#3ddc84";
const UP_COLOR = "#5ad1ff";
const DOWN_COLOR = "#ffa94d";
const TEXT_COLOR = "#eef2f7";
const HINT_COLOR = "#c9d1dc";

let settings: RadarSettings = DEFAULT_SETTINGS;
let frame: RadarFrame | null = null;
let frameAt = 0;
let camera: { deg: number; at: number } | null = null;
/** The cube pointed at, and since when (a new one gets a pulse). */
let target: { id: string; since: number } | null = null;

function resize(): void {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
  const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
}

/** The camera direction: the minimap's view cone, else the travel direction. */
function heading(now: number): number | null {
  if (camera && now - camera.at < 1000) return camera.deg;
  return frame?.player?.headingDeg ?? null;
}

interface Geometry {
  cx: number;
  cy: number;
  rx: number;
  ry: number;
  /** 1 at the default ring size. */
  s: number;
  w: number;
  h: number;
}

function render(): void {
  const now = performance.now();
  resize();
  const { width: w, height: h } = canvas;
  ctx.clearRect(0, 0, w, h);
  const player = frame?.player ?? null;
  const fresh = frame !== null && player !== null && now - frameAt < 3000;
  const cube = fresh && frame ? nearestLive(frame.cubes) : null;
  if (!cube || cube.distance === null || cube.bearingDeg === null || !player) {
    target = null;
    requestAnimationFrame(render);
    return;
  }
  if (target?.id !== cube.id) target = { id: cube.id, since: now };

  const ring = w / AREA_W;
  const g: Geometry = { cx: w / 2, cy: h / 2, rx: ring * 0.3, ry: ring * 0.18, s: ring / 260, w, h };
  const cue = heightCue(cube.height, player.heightAccuracyM);
  const cam = heading(now);
  // An old position: still shown, faded, and never as "here".
  ctx.globalAlpha = player.stale ? 0.45 : 1;
  if (!player.stale && isHere(cube.distance, player.accuracyM)) {
    drawHere(g, cue, cube.distance, now);
  } else {
    if (now - target.since < 1600) drawNewPulse(g, now - target.since);
    drawRing(g);
    const lines = labelLines(cube.distance, cue, player.stale);
    if (cam === null) {
      // No camera direction: no arrow, just the label above the ring.
      drawLabel(g, lines, g.cx, g.cy - g.ry - 12 * g.s, 0, -1);
    } else {
      const p = ringPoint(relativeBearing(cube.bearingDeg, cam), g.cx, g.cy, g.rx, g.ry);
      drawArrow(g, p);
      // Just past the arrow's tip, on the far side of it.
      const ux = Math.sin(p.angle);
      const uy = -Math.cos(p.angle);
      drawLabel(g, lines, p.x + ux * 40 * g.s, p.y + uy * 40 * g.s, ux, uy);
    }
  }
  ctx.globalAlpha = 1;
  requestAnimationFrame(render);
}

interface Line {
  text: string;
  color: string;
  /** Font size at the default ring size. */
  size: number;
}

function labelLines(distance: number, cue: HeightCue, stale: boolean): Line[] {
  const lines: Line[] = [{ text: formatMetres(distance), color: settings.colors.cube, size: 22 }];
  const h = heightText(cue);
  if (h) lines.push({ text: h, color: cueColor(cue), size: 16 });
  else if (distance <= HEIGHT_HINT_M) lines.push(...WINGS_HINT);
  if (stale) lines.push({ text: "your position is old", color: HINT_COLOR, size: 12 });
  return lines;
}

/** Your own height is not sure enough to tell: a server fix measures it. */
const WINGS_HINT: readonly Line[] = [
  { text: "up or down?", color: HINT_COLOR, size: 13 },
  { text: "open your wings once", color: HINT_COLOR, size: 13 },
];

function cueColor(cue: HeightCue): string {
  return cue.kind === "up" ? UP_COLOR : cue.kind === "down" ? DOWN_COLOR : TEXT_COLOR;
}

/** The ring on the ground. */
function drawRing(g: Geometry): void {
  ctx.save();
  ctx.beginPath();
  ctx.ellipse(g.cx, g.cy, g.rx, g.ry, 0, 0, Math.PI * 2);
  ctx.setLineDash([7 * g.s, 7 * g.s]);
  ctx.strokeStyle = "rgba(0,0,0,0.45)";
  ctx.lineWidth = 5 * g.s;
  ctx.stroke();
  ctx.strokeStyle = withAlpha(settings.colors.cube, 0.6);
  ctx.lineWidth = 2.5 * g.s;
  ctx.stroke();
  ctx.restore();
}

/** The arrow on the ring (at `p`), pointing outwards towards the cube. */
function drawArrow(g: Geometry, p: { x: number; y: number; angle: number }): void {
  const s = g.s;
  ctx.save();
  ctx.translate(p.x, p.y);
  ctx.rotate(p.angle);
  ctx.shadowColor = "rgba(0,0,0,0.85)";
  ctx.shadowBlur = 10 * s;
  ctx.beginPath();
  ctx.moveTo(0, -30 * s);
  ctx.lineTo(22 * s, 10 * s);
  ctx.lineTo(0, 1 * s);
  ctx.lineTo(-22 * s, 10 * s);
  ctx.closePath();
  ctx.fillStyle = settings.colors.cube;
  ctx.fill();
  ctx.shadowBlur = 0;
  ctx.lineWidth = 2.5 * s;
  ctx.strokeStyle = "rgba(0,0,0,0.9)";
  ctx.stroke();
  ctx.restore();
}

/** Rings spreading out from the character: a new cube to go for. */
function drawNewPulse(g: Geometry, age: number): void {
  for (const delay of [0, 400]) {
    const t = (age - delay) / 1200;
    if (t < 0 || t > 1) continue;
    const k = 1 + 0.9 * t;
    ctx.save();
    ctx.beginPath();
    ctx.ellipse(g.cx, g.cy, g.rx * k, g.ry * k, 0, 0, Math.PI * 2);
    ctx.strokeStyle = withAlpha(settings.colors.cube, 0.8 * (1 - t));
    ctx.lineWidth = 4 * g.s;
    ctx.stroke();
    ctx.restore();
  }
}

/** On top of the cube: a pulsing green circle and where to look. */
function drawHere(g: Geometry, cue: HeightCue, distance: number, now: number): void {
  const beat = (now % 900) / 900;
  const k = 1 + 0.06 * Math.sin(beat * Math.PI * 2);
  ctx.save();
  // A ripple going out, then the circle itself.
  const t = (now % 1300) / 1300;
  ctx.beginPath();
  ctx.ellipse(g.cx, g.cy, g.rx * (1 + 0.5 * t), g.ry * (1 + 0.5 * t), 0, 0, Math.PI * 2);
  ctx.strokeStyle = withAlpha(HERE_COLOR, 0.7 * (1 - t));
  ctx.lineWidth = 3 * g.s;
  ctx.stroke();
  ctx.beginPath();
  ctx.ellipse(g.cx, g.cy, g.rx * k, g.ry * k, 0, 0, Math.PI * 2);
  ctx.fillStyle = withAlpha(HERE_COLOR, 0.14);
  ctx.fill();
  ctx.strokeStyle = "rgba(0,0,0,0.5)";
  ctx.lineWidth = 7 * g.s;
  ctx.stroke();
  ctx.strokeStyle = HERE_COLOR;
  ctx.lineWidth = 4 * g.s;
  ctx.stroke();
  ctx.restore();
  const lines: Line[] = [{ text: "CUBE HERE", color: HERE_COLOR, size: 24 }];
  switch (cue.kind) {
    case "up":
      lines.push({ text: `▲ look up · ${cue.approx ? "~" : ""}${cue.metres} m`, color: UP_COLOR, size: 18 });
      break;
    case "down":
      lines.push({ text: `▼ look down · ${cue.approx ? "~" : ""}${cue.metres} m`, color: DOWN_COLOR, size: 18 });
      break;
    case "level":
      lines.push({ text: `look around you · ${formatMetres(distance)}`, color: TEXT_COLOR, size: 16 });
      break;
    case "unknown":
      lines.push(...WINGS_HINT);
      break;
  }
  drawLabel(g, lines, g.cx, g.cy - g.ry * 1.06 - 12 * g.s, 0, -1);
}

/**
 * Lines of text on a dark rounded box, kept on screen. The box sits on the
 * (ux, uy) side of (x, y): (0, -1) puts it above the point, (1, 0) to its
 * right; (0, 0) centres it there.
 */
function drawLabel(g: Geometry, lines: readonly Line[], x: number, y: number, ux = 0, uy = 0): void {
  const s = g.s;
  const font = (l: Line) => `800 ${Math.round(l.size * s)}px system-ui, 'Segoe UI', sans-serif`;
  let width = 0;
  let height = 0;
  for (const l of lines) {
    ctx.font = font(l);
    width = Math.max(width, ctx.measureText(l.text).width);
    height += l.size * 1.2 * s;
  }
  const pad = 8 * s;
  const bw = width + 2 * pad;
  const bh = height + pad;
  const left = Math.min(g.w - bw - 2, Math.max(2, x + ((ux - 1) * bw) / 2));
  const top = Math.min(g.h - bh - 2, Math.max(2, y + ((uy - 1) * bh) / 2));
  ctx.save();
  ctx.lineJoin = "round";
  ctx.beginPath();
  ctx.roundRect(left, top, bw, bh, 8 * s);
  ctx.fillStyle = "rgba(8,12,20,0.62)";
  ctx.fill();
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  let ly = top + pad / 2;
  for (const l of lines) {
    const lh = l.size * 1.2 * s;
    ctx.font = font(l);
    ctx.lineWidth = 4 * s;
    ctx.strokeStyle = "rgba(0,0,0,0.85)";
    ctx.strokeText(l.text, left + bw / 2, ly + lh / 2);
    ctx.fillStyle = l.color;
    ctx.fillText(l.text, left + bw / 2, ly + lh / 2);
    ly += lh;
  }
  ctx.restore();
}

async function main(): Promise<void> {
  try {
    settings = await backend.getSettings();
  } catch (e) {
    console.error(e);
  }
  await backend.onSettings((s) => (settings = s));
  await backend.onFrame((f) => {
    frame = f;
    frameAt = performance.now();
  });
  window.setInterval(async () => {
    try {
      const v = await backend.getMinimapView();
      camera = v && v.cameraDeg !== null ? { deg: v.cameraDeg, at: performance.now() } : null;
    } catch {
      camera = null;
    }
  }, 100);
  requestAnimationFrame(render);
}

main().catch((e) => console.error(e));
