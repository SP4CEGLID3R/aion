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
  if (s.dataSource.kind !== "packet_capture") return 'Works when "Cubes come from" is the game\'s network traffic.';
  if (!wantsMinimap(s)) return "Needs the AION 2 decoder on live traffic (More settings → Game traffic).";
  if (!vision) return "Starting…";
  if (vision.error) return `Screen capture failed: ${vision.error}`;
  if (vision.black)
    return "The minimap area is black: run the game in borderless or windowed mode, not exclusive fullscreen.";
  if (vision.frozen) return "The picture is not changing: is the game minimised, or is the area wrong?";
  const m = capture?.aion2?.minimap;
  if (!m) return "Waiting for the game connection…";
  switch (m.state) {
    case "no frames":
      return "Waiting for the first picture…";
    case "no fix":
      return "Open your wings once so the radar knows where you are.";
    case "tracking":
      return m.legs >= 3
        ? "Following you ✓"
        : "Following you ✓ (opening your wings a couple more times as you go fine-tunes the map scale)";
    case "recalibrating":
      return "The minimap zoom changed: open your wings after walking a bit, to re-measure.";
    default:
      return `Not sure right now: ${m.reason || "waiting"}.`;
  }
}

