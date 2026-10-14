// Browser-only preview frames, for looking at the UI with `pnpm dev`.
//
// This does NOT run the Rust tracker; it fabricates a few frames that are
// already in the shape the tracker emits, and says so in the source name.
// Real (synthetic or telemetry) data only flows inside the Tauri app.

import type { CubeState, RadarFrame, RadarSettings } from "./types";

export function startPreviewFrames(cb: (f: RadarFrame) => void, settings: () => RadarSettings): () => void {
  const start = Date.now();
  const cubes = [
    { id: "preview-1", label: "Cube #1", x: 40, y: 55 },
    { id: "preview-2", label: "Cube #2", x: -70, y: 20 },
    { id: "preview-3", label: "Cube #3", x: 15, y: -95 },
    { id: "preview-4", label: "Cube #4", x: 260, y: 140 },
    { id: "preview-5", label: "Cube #5", x: -30, y: -40, stale: true },
  ];
  const tick = () => {
    const now = Date.now();
    const s = (now - start) / 1000;
    const px = 20 * Math.sin(s / 6);
    const py = 20 * Math.cos(s / 9);
    const heading = (((Math.atan2(Math.cos(s / 6) / 6, -Math.sin(s / 9) / 9) * 180) / Math.PI) + 360) % 360;
    const showStale = settings().staleDisplay === "mark_stale";
    cb({
      generatedAt: now,
      zone: "preview",
      player: { pos: { x: px, y: py, z: 0 }, headingDeg: heading, ageMs: 0, stale: false },
      cubes: cubes
        .filter((c) => !c.stale || showStale)
        .map((c) => ({
          id: c.id,
          label: c.label,
          state: (c.stale ? "STALE" : "SPAWNED") as CubeState,
          pos: { x: c.x, y: c.y, z: 0 },
          distance: Math.hypot(c.x - px, c.y - py),
          bearingDeg: null,
          confirmedAgeMs: c.stale ? 14_000 : 400,
        })),
      stats: { spawned: 4, stale: 1, tracked: 5, accepted: 0, duplicates: 0, rejected: 0 },
      source: {
        name: "Browser preview (fabricated frames)",
        state: "connected",
        detail: "not the Rust pipeline",
        lastEventAt: now,
      },
    });
  };
  tick();
  const id = window.setInterval(tick, 50);
  return () => window.clearInterval(id);
}
