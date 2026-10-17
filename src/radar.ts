// The radar window: receives frames, interpolates between them, draws at the
// display's refresh rate. Rendering never waits on the backend.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { backend } from "./lib/bridge";
import { beepPitch, isHere, nearestLive, ProximityBeeper } from "./lib/guide";
import { lerp, lerpAngleDeg, viewUpDeg, type View } from "./lib/projection";
import { drawScene, type MinimapBackground } from "./lib/renderer";
import { AutoHide } from "./lib/autoHide";
import { AlertGate, playAlert, playArrived, playBeep } from "./lib/sound";
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
const beeper = new ProximityBeeper();
const autoHide = new AutoHide();
/** What the window was last told: shown, or invisible and click-through. */
let radarShown = true;

// The live game minimap, drawn behind the radar.
let background: MinimapBackground | null = null;
let backgroundSeq = 0;
let backgroundAt = 0;
let backgroundBusy = false;
let backgroundCanvas: HTMLCanvasElement | null = null;

async function pollMinimap(): Promise<void> {
  const wanted =
    !backend.isPreview && settings.minimap.enabled && settings.minimap.background && settings.style === "radar";
  if (!wanted) {
    background = null;
    return;
  }
  // Never draw a picture that stopped updating.
  if (background && performance.now() - backgroundAt > 2000) background = null;
  if (backgroundBusy) return;
  backgroundBusy = true;
  try {
    const f = await backend.minimapFrame(backgroundSeq);
    if (f) {
      backgroundSeq = f.seq;
      backgroundCanvas ??= document.createElement("canvas");
      if (backgroundCanvas.width !== f.w || backgroundCanvas.height !== f.h) {
        backgroundCanvas.width = f.w;
        backgroundCanvas.height = f.h;
      }
      backgroundCanvas.getContext("2d")!.putImageData(new ImageData(f.rgba, f.w, f.h), 0, 0);
      background =
        f.pxPerM > 0 ? { image: backgroundCanvas, pivotX: f.pivotX, pivotY: f.pivotY, pxPerM: f.pxPerM } : null;
      backgroundAt = performance.now();
    }
  } catch (e) {
    console.error("minimap frame", e);
  } finally {
    backgroundBusy = false;
  }
}

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

/** Soft beeps towards the nearest cube, while your position is current. */
function maybeBeep(frame: RadarFrame | null, backendSilent: boolean, now: number): void {
  const p = frame?.player;
  const usable =
    settings.guide.beep && !backendSilent && p && !p.stale && (p.accuracyM === null || p.accuracyM <= 15);
  const cube = usable && frame ? nearestLive(frame.cubes) : null;
  const distance = cube?.distance ?? null;
  const here = distance !== null && isHere(distance, p?.accuracyM ?? null);
  const beep = beeper.step(cube?.id ?? null, distance, here, now);
  if (!beep || distance === null) return;
  try {
    if (beep === "arrived") playArrived(settings.guide.beepVolume);
    else playBeep(settings.guide.beepVolume, beepPitch(distance));
  } catch (e) {
    console.error("beep", e);
  }
}

let settingsLoaded = false;

function applySettings(s: RadarSettings): void {
  const switched = settingsLoaded && s.hideWhenEmpty !== settings.hideWhenEmpty;
  settings = s;
  settingsLoaded = true;
  if (switched && s.windowMode === "overlay") {
    // Shown for a moment either way, so the switch (Ctrl+Tab) is seen.
    autoHide.reveal(performance.now(), 2000);
    showToast(s.hideWhenEmpty ? "Radar shows only when a cube is on it" : "Radar always shown", 2000);
  }
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
  const cubesDrawn = drawScene(
    ctx,
    {
      settings,
      player: pose.pos,
      headingDeg: pose.heading,
      cubes: backendSilent || !frame ? [] : frame.cubes,
      view,
      accuracyM: backendSilent ? null : (frame?.player?.accuracyM ?? null),
      trail: trail.map((t) => t.pos).concat(pose.pos ? [pose.pos] : []),
      background: backendSilent ? null : background,
    },
    size.w,
    size.h,
  );

  setBanner(bannerText(frame, backendSilent));
  if (!backend.isPreview) maybeBeep(frame, backendSilent, now);
  showRadar(autoHide.visible(autoHideOn(), cubesDrawn, now));
  requestAnimationFrame(render);
}

/** "Show the radar only when a cube is on it" applies to the overlay. */
function autoHideOn(): boolean {
  return settings.hideWhenEmpty && settings.windowMode === "overlay" && !backend.isPreview;
}

/** Invisible windows keep drawing (sounds and beeps go on) but let clicks
 * through, so the game under them can be clicked. */
function showRadar(shown: boolean): void {
  if (shown === radarShown) return;
  radarShown = shown;
  document.body.classList.toggle("auto-hidden", !shown);
  backend.setRadarHidden(!shown).catch((e) => console.error("radar visibility", e));
}

let toastTimer = 0;
function showToast(message: string, ms = 1500): void {
  const toast = document.querySelector<HTMLElement>("#toast")!;
  toast.textContent = message;
  toast.hidden = false;
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.hidden = true), ms);
}

function bannerText(frame: RadarFrame | null, backendSilent: boolean): string | null {
  if (!frame || backendSilent) return "No data from the radar backend";
  const src = frame.source;
  if (src?.state === "error") return `Data source error: ${src.detail}`;
  if (src?.state === "disabled") return "No data source configured";
  if (!frame.player) {
    if (src?.state === "waiting") return `Waiting for telemetry — ${src.detail}`;
    // Live capture: the server reports your own position only with events,
    // and opening the wings is the one a player can do at any time.
    if (src?.state === "connected" && /^Packet capture/.test(src.name) && !/census only/.test(src.detail)) {
      return "Waiting for your position — open your wings once in the game";
    }
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
  const n = frame.stats.spawned;
  const parts = [
    n === 0 ? "no cubes nearby" : n === 1 ? "1 cube" : `${n} cubes`,
    frame.player?.accuracyM ? `position ±${Math.max(1, Math.round(frame.player.accuracyM))} m` : null,
    // The source only when something is off with it.
    src && src.state !== "connected" ? `${src.name} (${src.state})` : null,
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
  if (autoHideOn()) {
    // Seen once at start, so an empty radar does not look like a radar that
    // did not start.
    autoHide.reveal(performance.now(), 3000);
    showToast("Radar shows only when a cube is on it (Ctrl+Tab)", 3000);
  }
  await backend.onSettings(applySettings);
  await backend.onFrame((frame) => {
    prev = cur;
    cur = { frame, at: performance.now() };
    updateStatus(frame);
    recordTrail(frame, cur.at);
    maybeAlert(frame, cur.at);
  });

  await backend.onMarker((message) => showToast(message));

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

  window.setInterval(() => void pollMinimap(), 100);
  requestAnimationFrame(render);
}

main().catch((e) => {
  console.error(e);
  setBanner(`Radar failed to start: ${String(e)}`);
});
