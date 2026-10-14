// The radar window: receives frames, interpolates between them, draws at the
// display's refresh rate. Rendering never waits on the backend.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { backend } from "./lib/bridge";
import { lerp, lerpAngleDeg, viewUpDeg, type View } from "./lib/projection";
import { drawScene } from "./lib/renderer";
import { AlertGate, playAlert } from "./lib/sound";
import { DEFAULT_SETTINGS, type RadarFrame, type RadarSettings, type WorldPos } from "./lib/types";

const canvas = document.querySelector<HTMLCanvasElement>("#radar")!;
const ctx = canvas.getContext("2d")!;
const root = document.querySelector<HTMLElement>("#root")!;
const banner = document.querySelector<HTMLElement>("#banner")!;
const statusEl = document.querySelector<HTMLElement>("#status")!;
const badge = document.querySelector<HTMLElement>("#badge")!;
const legend = document.querySelector<HTMLElement>("#legend")!;
const legendStale = document.querySelector<HTMLElement>("#legend-stale")!;
const legendArrow = document.querySelector<HTMLElement>("#legend-arrow")!;

let settings: RadarSettings = DEFAULT_SETTINGS;
let prev: { frame: RadarFrame; at: number } | null = null;
let cur: { frame: RadarFrame; at: number } | null = null;
let size = { w: 0, h: 0, dpr: 1 };
const alertGate = new AlertGate();

/** Own positions of the last minute, for the trail. */
const TRAIL_MS = 60_000;
let trail: { at: number; pos: WorldPos }[] = [];
let trailZone: string | null = null;

function recordTrail(frame: RadarFrame, now: number): void {
  if (frame.zone !== trailZone) {
    trailZone = frame.zone;
    trail = [];
  }
  const p = frame.player?.pos;
  if (!p) return;
  const last = trail[trail.length - 1];
  const moved = last ? Math.hypot(p.x - last.pos.x, p.y - last.pos.y) : Infinity;
  // A jump this far is a teleport, not a walk: start a new trail.
  if (moved > 300) trail = [];
  if (moved >= 1 || !last) trail.push({ at: now, pos: p });
  while (trail.length > 0 && now - trail[0]!.at > TRAIL_MS) trail.shift();
  if (trail.length > 600) trail.splice(0, trail.length - 600);
}

function maybeAlert(frame: RadarFrame, now: number): void {
  const live = frame.cubes.filter((c) => c.state === "SPAWNED").map((c) => c.id);
  if (alertGate.check(frame.zone, live, now) && settings.alertSound.enabled) {
    playAlert(settings.alertSound).catch((e) => console.error("alert", e));
  }
}

function applySettings(s: RadarSettings): void {
  settings = s;
  root.style.opacity = String(s.opacity);
  legend.hidden = !s.showLegend;
  legendStale.hidden = s.staleDisplay !== "mark_stale";
  legendArrow.hidden = !s.showIndicators;
  document.body.classList.toggle("separate", s.windowMode === "separate");
}

function resize(): void {
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  size = { w: rect.width, h: rect.height, dpr };
  canvas.width = Math.max(1, Math.round(rect.width * dpr));
  canvas.height = Math.max(1, Math.round(rect.height * dpr));
}

function setBanner(text: string | null): void {
  banner.hidden = text === null;
  if (text !== null && banner.textContent !== text) banner.textContent = text;
}

/** Player pose interpolated between the last two frames, one frame behind. */
function playerPose(now: number): { pos: WorldPos | null; heading: number | null } {
  const p1 = cur?.frame.player;
  if (!cur || !p1) return { pos: null, heading: null };
  const p0 = prev?.frame.player;
  if (!prev || !p0) return { pos: p1.pos, heading: p1.headingDeg };
  const span = Math.max(1, cur.at - prev.at);
  const t = Math.min(1, Math.max(0, (now - cur.at) / span));
  const pos = { x: lerp(p0.pos.x, p1.pos.x, t), y: lerp(p0.pos.y, p1.pos.y, t), z: lerp(p0.pos.z, p1.pos.z, t) };
  const heading =
    p0.headingDeg !== null && p1.headingDeg !== null ? lerpAngleDeg(p0.headingDeg, p1.headingDeg, t) : p1.headingDeg;
  return { pos, heading };
}

function render(): void {
  const now = performance.now();
  const frame = cur?.frame ?? null;
  // If the backend stops sending frames, do not keep drawing the last ones
  // as if they were current.
  const frameAgeMs = cur ? now - cur.at : Infinity;
  const backendSilent = frameAgeMs > settings.staleTimeoutSecs * 1000;

  const pose = backendSilent ? { pos: null, heading: null } : playerPose(now);
  const view: View = {
    cx: size.w / 2,
    cy: size.h / 2,
    radiusPx: Math.max(10, Math.min(size.w, size.h) / 2 - 6),
    range: settings.range,
    upDeg: viewUpDeg(settings.rotationMode, settings.rotationOffsetDeg, pose.heading),
  };

  ctx.setTransform(size.dpr, 0, 0, size.dpr, 0, 0);
  drawScene(
    ctx,
    {
      settings,
      player: pose.pos,
      headingDeg: pose.heading,
      cubes: backendSilent || !frame ? [] : frame.cubes,
      view,
      accuracyM: backendSilent ? null : (frame?.player?.accuracyM ?? null),
      trail: trail.map((t) => t.pos).concat(pose.pos ? [pose.pos] : []),
    },
    size.w,
    size.h,
  );

  setBanner(bannerText(frame, backendSilent));
  requestAnimationFrame(render);
}

function bannerText(frame: RadarFrame | null, backendSilent: boolean): string | null {
  if (!frame || backendSilent) return "No data from the radar backend";
  const src = frame.source;
  if (src?.state === "error") return `Data source error: ${src.detail}`;
  if (src?.state === "disabled") return "No data source configured";
  if (!frame.player) {
    if (src?.state === "waiting") return `Waiting for telemetry — ${src.detail}`;
    return src?.detail ? `Waiting for player position — ${src.detail}` : "Waiting for player position";
  }
  if (frame.player.stale) {
    const age = Math.round(frame.player.ageMs / 1000);
    return `Your position is ${age} s old — open your wings to update the position`;
  }
  if (src?.state === "finished") return "Replay finished";
  return null;
}

function updateStatus(frame: RadarFrame): void {
  const src = frame.source;
  const parts = [
    `${frame.stats.spawned} live`,
    frame.stats.stale ? `${frame.stats.stale} stale` : null,
    frame.zone ? `zone ${frame.zone}` : null,
    frame.player?.accuracyM ? `position estimated ±${Math.max(1, Math.round(frame.player.accuracyM))} m` : null,
    src ? `${src.name} (${src.state})` : null,
  ].filter(Boolean);
  const text = parts.join(" · ");
  if (statusEl.textContent !== text) statusEl.textContent = text;

  const synthetic = backend.isPreview || /synthetic|preview/i.test(src?.name ?? "");
  badge.hidden = !synthetic;
  badge.textContent = backend.isPreview ? "PREVIEW" : "SYNTHETIC";
  badge.classList.toggle("preview", backend.isPreview);
  badge.title = backend.isPreview
    ? "Browser preview: fabricated frames, the Rust pipeline is not running"
    : "Synthetic data source: simulated cubes, not game data";
}

async function main(): Promise<void> {
  new ResizeObserver(resize).observe(canvas);
  resize();

  try {
    applySettings(await backend.getSettings());
  } catch (e) {
    console.error("could not load settings", e);
  }
  await backend.onSettings(applySettings);
  await backend.onFrame((frame) => {
    prev = cur;
    cur = { frame, at: performance.now() };
    updateStatus(frame);
    recordTrail(frame, cur.at);
    maybeAlert(frame, cur.at);
  });

  let toastTimer = 0;
  await backend.onMarker((message) => {
    const toast = document.querySelector<HTMLElement>("#toast")!;
    toast.textContent = message;
    toast.hidden = false;
    window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => (toast.hidden = true), 1500);
  });

  document.querySelector("#btn-settings")!.addEventListener("click", () => {
    backend.openSettingsWindow().catch((e) => console.error(e));
  });
  document.querySelector("#resize-grip")!.addEventListener("mousedown", (e) => {
    if (backend.isPreview) return;
    e.preventDefault();
    getCurrentWindow()
      .startResizeDragging("SouthEast")
      .catch((err) => console.error(err));
  });

  requestAnimationFrame(render);
}

main().catch((e) => {
  console.error(e);
  setBanner(`Radar failed to start: ${String(e)}`);
});
