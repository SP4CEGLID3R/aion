// The arrow around the character, drawn over the game: a ring on the ground
// with an arrow towards the nearest live cube, turned with the camera (from
// the minimap's view cone) so "ahead" is where you look.

import { backend } from "./lib/bridge";
import { formatMetres, nearestLive, relativeBearing, ringPoint } from "./lib/guide";
import { withAlpha } from "./lib/renderer";
import { DEFAULT_SETTINGS, type RadarFrame, type RadarSettings } from "./lib/types";

const canvas = document.querySelector<HTMLCanvasElement>("#guide")!;
const ctx = canvas.getContext("2d")!;

let settings: RadarSettings = DEFAULT_SETTINGS;
let frame: RadarFrame | null = null;
let frameAt = 0;
let camera: { deg: number; at: number } | null = null;

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

function render(): void {
  const now = performance.now();
  resize();
  const { width: w, height: h } = canvas;
  ctx.clearRect(0, 0, w, h);
  const fresh = frame && now - frameAt < 3000;
  const target = fresh && frame ? nearestLive(frame.cubes) : null;
  const cam = heading(now);
  if (target && target.bearingDeg !== null && target.distance !== null && cam !== null) {
    draw(relativeBearing(target.bearingDeg, cam), target.distance, w, h);
  }
  requestAnimationFrame(render);
}

function draw(rel: number, distance: number, w: number, h: number): void {
  const color = settings.colors.cube;
  const s = Math.min(w, h / 0.75) / 260; // 1 at the default size
  const cx = w / 2;
  const cy = h / 2;
  // Leave room around the ring for the arrow and the distance.
  const rx = w * 0.3;
  const ry = h * 0.24;
  // The ring on the ground.
  ctx.save();
  ctx.beginPath();
  ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
  ctx.setLineDash([6 * s, 8 * s]);
  ctx.strokeStyle = withAlpha(color, 0.35);
  ctx.lineWidth = 2 * s;
  ctx.stroke();
  ctx.restore();
  // The arrow on the ring, pointing outwards towards the cube.
  const p = ringPoint(rel, cx, cy, rx, ry);
  ctx.save();
  ctx.translate(p.x, p.y);
  ctx.rotate(p.angle);
  ctx.shadowColor = "rgba(0,0,0,0.8)";
  ctx.shadowBlur = 8 * s;
  ctx.beginPath();
  ctx.moveTo(0, -22 * s);
  ctx.lineTo(16 * s, 8 * s);
  ctx.lineTo(0, 1 * s);
  ctx.lineTo(-16 * s, 8 * s);
  ctx.closePath();
  ctx.fillStyle = color;
  ctx.fill();
  ctx.lineWidth = 2 * s;
  ctx.strokeStyle = "rgba(0,0,0,0.85)";
  ctx.stroke();
  ctx.restore();
  // Distance, just outside the arrow.
  const t = ringPoint(rel, cx, cy, rx + 36 * s, ry + 30 * s);
  ctx.save();
  ctx.font = `700 ${Math.round(15 * s)}px system-ui, 'Segoe UI', sans-serif`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.lineWidth = 4 * s;
  ctx.strokeStyle = "rgba(0,0,0,0.85)";
  const text = formatMetres(distance);
  ctx.strokeText(text, t.x, t.y);
  ctx.fillStyle = color;
  ctx.fillText(text, t.x, t.y);
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
  }, 150);
  requestAnimationFrame(render);
}

main().catch((e) => console.error(e));
