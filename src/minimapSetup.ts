// Settings window: the minimap tracking section (live preview, status in
// plain words) and the picker for where the minimap is on screen.

import { backend } from "./lib/bridge";
import { minimapStatusText } from "./lib/minimapText";
import type {
  CaptureStatus,
  MonitorInfo,
  RadarSettings,
  ScreenRegion,
  Screenshot,
  VisionStatus,
} from "./lib/types";

interface Hooks {
  settings: () => RadarSettings | null;
  apply: (s: RadarSettings) => Promise<void>;
  warn: (lines: string[]) => void;
}

const $ = <T extends Element>(sel: string) => document.querySelector<T>(sel)!;

// ---------------------------------------------------------------------------
// Live preview and status
// ---------------------------------------------------------------------------

let previewSeq = 0;
let previewBusy = false;

async function pollPreview(): Promise<void> {
  if (previewBusy || document.visibilityState !== "visible") return;
  previewBusy = true;
  try {
    const f = await backend.minimapFrame(previewSeq);
    if (!f) return;
    previewSeq = f.seq;
    const c = $<HTMLCanvasElement>("#minimap-preview");
    if (c.width !== f.w || c.height !== f.h) {
      c.width = f.w;
      c.height = f.h;
    }
    const ctx = c.getContext("2d")!;
    ctx.putImageData(new ImageData(f.rgba, f.w, f.h), 0, 0);
    cross(ctx, f.pivotX, f.pivotY, 9);
  } catch (e) {
    console.error("minimap preview", e);
  } finally {
    previewBusy = false;
  }
}

function cross(ctx: CanvasRenderingContext2D, x: number, y: number, r: number): void {
  ctx.save();
  ctx.lineWidth = 3;
  ctx.strokeStyle = "rgba(0,0,0,0.8)";
  ctx.beginPath();
  ctx.moveTo(x - r, y);
  ctx.lineTo(x + r, y);
  ctx.moveTo(x, y - r);
  ctx.lineTo(x, y + r);
  ctx.stroke();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#ff4d6d";
  ctx.stroke();
  ctx.restore();
}

export function renderMinimapStatus(
  s: RadarSettings | null,
  vision: VisionStatus | null,
  capture: CaptureStatus | null,
): void {
  $<HTMLElement>("#minimap-status").textContent = minimapStatusText(s, vision, capture);
  const parts: string[] = [];
  if (vision?.region) {
    const r = vision.region;
    parts.push(vision.defaultRegion ? "Reading the usual spot." : `Reading the area you picked (${r.width}×${r.height}).`);
  }
  if (vision?.running) {
    parts.push(
      vision.coneClear ? "Camera direction: seen." : "Camera direction: not visible yet (the arrow over the game needs it).",
    );
  }
  $<HTMLElement>("#minimap-region-text").textContent = parts.join(" ");
}

// ---------------------------------------------------------------------------
// Picker
// ---------------------------------------------------------------------------

let monitors: MonitorInfo[] = [];
let shot: Screenshot | null = null;
const shotCanvas = document.createElement("canvas");
/** Selection in screenshot pixels. */
let sel: { x: number; y: number; w: number; h: number } | null = null;
let dragFrom: { x: number; y: number } | null = null;

function monitorOf(r: ScreenRegion | null): number {
  if (r) {
    const cx = r.x + r.width / 2;
    const cy = r.y + r.height / 2;
    const i = monitors.findIndex((m) => cx >= m.x && cy >= m.y && cx < m.x + m.width && cy < m.y + m.height);
    if (i >= 0) return i;
  }
  const p = monitors.findIndex((m) => m.primary);
  return p >= 0 ? p : 0;
}

function drawPicker(): void {
  const c = $<HTMLCanvasElement>("#picker-canvas");
  const ctx = c.getContext("2d")!;
  if (!shot) {
    c.width = 640;
    c.height = 360;
    ctx.fillStyle = "#0b1220";
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.fillStyle = "#9aa6b8";
    ctx.font = "16px system-ui, sans-serif";
    ctx.textAlign = "center";
    ctx.fillText("Take a screenshot with the game showing", c.width / 2, c.height / 2);
    return;
  }
  if (c.width !== shot.w || c.height !== shot.h) {
    c.width = shot.w;
    c.height = shot.h;
  }
  ctx.drawImage(shotCanvas, 0, 0);
  if (sel) {
    // Dim everything but the selection.
    ctx.save();
    ctx.fillStyle = "rgba(0,0,0,0.55)";
    ctx.beginPath();
    ctx.rect(0, 0, c.width, c.height);
    ctx.rect(sel.x, sel.y, sel.w, sel.h);
    ctx.fill("evenodd");
    ctx.lineWidth = Math.max(2, shot.w / 640);
    ctx.strokeStyle = "#ffcc33";
    ctx.strokeRect(sel.x, sel.y, sel.w, sel.h);
    ctx.restore();
    cross(ctx, sel.x + sel.w / 2, sel.y + sel.h / 2, Math.max(8, shot.w / 160));
  }
  drawZoom();
  syncInputs();
}

function drawZoom(): void {
  const z = $<HTMLCanvasElement>("#picker-zoom");
  const ctx = z.getContext("2d")!;
  ctx.fillStyle = "#0b1220";
  ctx.fillRect(0, 0, z.width, z.height);
  if (!shot || !sel || sel.w < 2 || sel.h < 2) return;
  z.width = Math.round(sel.w);
  z.height = Math.round(sel.h);
  ctx.drawImage(shotCanvas, sel.x, sel.y, sel.w, sel.h, 0, 0, z.width, z.height);
  cross(ctx, z.width / 2, z.height / 2, 9);
}

function syncInputs(): void {
  if (!shot || !sel) return;
  $<HTMLInputElement>("#picker-x").value = String(Math.round(shot.x + sel.x));
  $<HTMLInputElement>("#picker-y").value = String(Math.round(shot.y + sel.y));
  $<HTMLInputElement>("#picker-w").value = String(Math.round(sel.w));
  $<HTMLInputElement>("#picker-h").value = String(Math.round(sel.h));
}

function fromInputs(): void {
  if (!shot) return;
  const n = (id: string) => Number($<HTMLInputElement>(id).value);
  sel = { x: n("#picker-x") - shot.x, y: n("#picker-y") - shot.y, w: n("#picker-w"), h: n("#picker-h") };
  drawPicker();
}

function toShot(e: MouseEvent): { x: number; y: number } {
  const c = $<HTMLCanvasElement>("#picker-canvas");
  const r = c.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(c.width, ((e.clientX - r.left) * c.width) / r.width)),
    y: Math.max(0, Math.min(c.height, ((e.clientY - r.top) * c.height) / r.height)),
  };
}

async function takeShot(): Promise<void> {
  const index = Number($<HTMLSelectElement>("#picker-monitor").value || 0);
  const count = $<HTMLElement>("#picker-count");
  if ($<HTMLInputElement>("#picker-delay").checked) {
    for (let i = 3; i > 0; i--) {
      count.textContent = `Switch to the game… ${i}`;
      await new Promise((r) => setTimeout(r, 1000));
    }
  }
  count.textContent = "Taking the screenshot…";
  shot = await backend.screenshotMonitor(index);
  shotCanvas.width = shot.w;
  shotCanvas.height = shot.h;
  shotCanvas.getContext("2d")!.putImageData(new ImageData(shot.rgba, shot.w, shot.h), 0, 0);
  count.textContent = "";
  // Start from the current area (or the default spot) on this monitor.
  let r = hooks?.settings()?.minimap.region ?? null;
  if (!r || monitorOf(r) !== index) r = await backend.defaultMinimapRegion(index);
  sel = { x: r.x - shot.x, y: r.y - shot.y, w: r.width, h: r.height };
  drawPicker();
}

let hooks: Hooks | null = null;

async function useRegion(region: ScreenRegion | null): Promise<void> {
  const s = hooks?.settings();
  if (!s || !hooks) return;
  const next = structuredClone(s);
  next.minimap.region = region;
  next.minimap.enabled = true;
  await hooks.apply(next);
  previewSeq = 0;
}

export function initMinimapSetup(h: Hooks): void {
  hooks = h;
  window.setInterval(() => void pollPreview(), 250);
  const dialog = $<HTMLDialogElement>("#picker");
  $<HTMLButtonElement>("#minimap-pick").addEventListener("click", async () => {
    try {
      monitors = await backend.listMonitors();
    } catch (e) {
      h.warn([String(e)]);
      return;
    }
    const select = $<HTMLSelectElement>("#picker-monitor");
    select.replaceChildren(
      ...monitors.map(
        (m, i) =>
          new Option(`${m.name || `Monitor ${i + 1}`} — ${m.width}×${m.height}${m.primary ? " (main)" : ""}`, String(i)),
      ),
    );
    select.value = String(monitorOf(h.settings()?.minimap.region ?? null));
    shot = null;
    sel = null;
    drawPicker();
    dialog.showModal();
  });
  $<HTMLButtonElement>("#minimap-default").addEventListener("click", () => {
    useRegion(null).catch((e) => h.warn([String(e)]));
  });
  $<HTMLButtonElement>("#picker-shot").addEventListener("click", () => {
    takeShot().catch((e) => {
      $<HTMLElement>("#picker-count").textContent = "";
      h.warn([`Screenshot failed: ${String(e)}`]);
    });
  });
  $<HTMLButtonElement>("#picker-cancel").addEventListener("click", () => dialog.close());
  $<HTMLButtonElement>("#picker-use").addEventListener("click", () => {
    if (!shot || !sel || sel.w < 40 || sel.h < 40) {
      h.warn(["Drag a box over the minimap first (at least 40×40 pixels)."]);
      return;
    }
    const region: ScreenRegion = {
      x: Math.round(shot.x + sel.x),
      y: Math.round(shot.y + sel.y),
      width: Math.round(sel.w),
      height: Math.round(sel.h),
    };
    dialog.close();
    useRegion(region).catch((e) => h.warn([String(e)]));
  });
  for (const id of ["#picker-x", "#picker-y", "#picker-w", "#picker-h"]) {
    $<HTMLInputElement>(id).addEventListener("change", fromInputs);
  }
  const canvas = $<HTMLCanvasElement>("#picker-canvas");
  canvas.addEventListener("mousedown", (e) => {
    if (!shot) return;
    dragFrom = toShot(e);
    sel = { ...dragFrom, w: 0, h: 0 };
  });
  window.addEventListener("mousemove", (e) => {
    if (!dragFrom || !shot) return;
    const p = toShot(e);
    sel = {
      x: Math.min(dragFrom.x, p.x),
      y: Math.min(dragFrom.y, p.y),
      w: Math.abs(p.x - dragFrom.x),
      h: Math.abs(p.y - dragFrom.y),
    };
    drawPicker();
  });
  window.addEventListener("mouseup", () => {
    dragFrom = null;
  });
}
