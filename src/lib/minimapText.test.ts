import { describe, expect, it } from "vitest";
import { minimapStatusText } from "./minimapText";
import { DEFAULT_SETTINGS, type CaptureStatus, type RadarSettings, type VisionStatus } from "./types";

const capture: RadarSettings = {
  ...DEFAULT_SETTINGS,
  dataSource: {
    kind: "packet_capture",
    device: null,
    file: null,
    decoder: "aion2",
    profilePath: null,
    north: "neg_y",
    includeUnconfirmed: false,
    record: false,
  },
};

const vision: VisionStatus = {
  running: true,
  region: { x: 2221, y: 126, width: 311, height: 206 },
  defaultRegion: true,
  uiScale: 1,
  frames: 100,
  grabMs: 1,
  trackMs: 6,
  black: false,
  frozen: false,
  breaks: 0,
  coneClear: true,
  error: null,
};

function withMinimap(state: string, legs = 0, reason = ""): CaptureStatus {
  return {
    aion2: {
      minimap: { state, reason, pxPerM: 1.03, axis: "right=+x, down=+y", legs },
    },
  } as unknown as CaptureStatus;
}

describe("minimap status text", () => {
  it("explains what is missing", () => {
    expect(minimapStatusText({ ...capture, minimap: { ...capture.minimap, enabled: false } }, vision, null)).toBe("Off.");
    expect(minimapStatusText(DEFAULT_SETTINGS, vision, null)).toMatch(/network traffic/);
    expect(minimapStatusText(capture, { ...vision, black: true }, null)).toMatch(/borderless/);
    expect(minimapStatusText(capture, { ...vision, error: "denied" }, null)).toMatch(/denied/);
    expect(minimapStatusText(capture, vision, withMinimap("no fix"))).toMatch(/open your wings once/i);
  });

  it("says whether the scale is confirmed", () => {
    expect(minimapStatusText(capture, vision, withMinimap("tracking", 0))).toMatch(/fine-tunes/);
    expect(minimapStatusText(capture, vision, withMinimap("tracking", 4))).toBe("Following you ✓");
    expect(minimapStatusText(capture, vision, withMinimap("uncertain", 0, "minimap hidden"))).toMatch(/minimap hidden/);
  });
});
