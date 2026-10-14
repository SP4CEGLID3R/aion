// What the minimap tracking is doing, in one plain sentence.

import type { CaptureStatus, RadarSettings, VisionStatus } from "./types";

export function wantsMinimap(s: RadarSettings | null): boolean {
  return (
    !!s &&
    s.minimap.enabled &&
    s.dataSource.kind === "packet_capture" &&
    s.dataSource.decoder === "aion2" &&
    !s.dataSource.file
  );
}

/** One plain sentence about what the minimap tracking is doing. */
export function minimapStatusText(
  s: RadarSettings | null,
  vision: VisionStatus | null,
  capture: CaptureStatus | null,
): string {
  if (!s?.minimap.enabled) return "Off.";
  if (!wantsMinimap(s)) return "Needs Data source → Packet capture (live) with the AION 2 decoder.";
  if (!vision) return "Starting…";
  if (vision.error) return `Screen capture failed: ${vision.error}`;
  if (vision.black)
    return "The minimap area is black: run the game in borderless or windowed mode, not exclusive fullscreen.";
  if (vision.frozen) return "The picture is not changing: is the game minimised, or is the area wrong?";
  const m = capture?.aion2?.minimap;
  if (!m) return "Waiting for the game connection…";
  const scale = m.pxPerM ? `${m.pxPerM.toFixed(2)} px/m` : "";
  switch (m.state) {
    case "no frames":
      return "Waiting for the first picture…";
    case "no fix":
      return "Open your wings once so the radar knows where you are.";
    case "tracking":
      return m.legs >= 3
        ? `Following you (scale ${scale}, confirmed).`
        : `Following you (default scale ${scale}; open your wings again after moving 10 m or more to confirm it).`;
    case "recalibrating":
      return "The minimap zoom seems to have changed: open your wings after moving, to re-measure.";
    default:
      return `Not sure right now: ${m.reason || "waiting"}.`;
  }
}

