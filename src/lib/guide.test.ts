import { describe, expect, it } from "vitest";
import { nearestLive, relativeBearing, ringPoint } from "./guide";
import type { CubeView } from "./types";

const cube = (id: string, state: CubeView["state"], distance: number | null, bearingDeg: number | null): CubeView => ({
  id,
  label: null,
  state,
  pos: { x: 0, y: 0, z: 0 },
  distance,
  bearingDeg,
  confirmedAgeMs: 0,
});

describe("guide", () => {
  it("points at the nearest live cube only", () => {
    expect(nearestLive([])).toBeNull();
    const c = nearestLive([cube("a", "SPAWNED", 50, 10), cube("b", "STALE", 5, 20), cube("c", "SPAWNED", 30, 90)]);
    expect(c?.id).toBe("c");
    expect(nearestLive([cube("d", "SPAWNED", null, null)])).toBeNull();
  });

  it("turns bearings into camera-relative directions", () => {
    expect(relativeBearing(90, 90)).toBe(0);
    expect(relativeBearing(0, 90)).toBe(270);
    expect(relativeBearing(180, 0)).toBe(180);
  });

  it("puts ahead at the top of the ring and right on the right", () => {
    const ahead = ringPoint(0, 100, 100, 80, 40);
    expect(ahead.x).toBeCloseTo(100);
    expect(ahead.y).toBeCloseTo(60);
    expect(ahead.angle).toBeCloseTo(0);
    const right = ringPoint(90, 100, 100, 80, 40);
    expect(right.x).toBeCloseTo(180);
    expect(right.y).toBeCloseTo(100);
    expect(right.angle).toBeCloseTo(Math.PI / 2);
  });
});
