import { describe, expect, it } from "vitest";
import {
  beepInterval,
  heightCue,
  heightText,
  isHere,
  nearestLive,
  ProximityBeeper,
  relativeBearing,
  ringPoint,
} from "./guide";
import type { CubeView } from "./types";

const cube = (id: string, state: CubeView["state"], distance: number | null, bearingDeg: number | null): CubeView => ({
  id,
  label: null,
  state,
  pos: { x: 0, y: 0, z: 0 },
  distance,
  bearingDeg,
  height: null,
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

  it("says up or down only when it is clearly so", () => {
    // Own height measured (right after a server fix).
    expect(heightCue(12.4, null)).toEqual({ kind: "up", metres: 12, approx: false });
    expect(heightCue(-5, null)).toEqual({ kind: "down", metres: 5, approx: false });
    expect(heightCue(1.5, null)).toEqual({ kind: "level" });
    // Moved away from the fix: the own height is unsure by 4 m.
    expect(heightCue(12, 4)).toEqual({ kind: "up", metres: 12, approx: true });
    expect(heightCue(6, 4)).toEqual({ kind: "unknown" });
    expect(heightCue(1, 4)).toEqual({ kind: "unknown" });
    expect(heightCue(null, null)).toEqual({ kind: "unknown" });
    expect(heightText(heightCue(12, 4))).toBe("▲ ~12 m up");
    expect(heightText(heightCue(-5, 0.5))).toBe("▼ 5 m down");
    expect(heightText(heightCue(0, 0.5))).toBe("same height");
    expect(heightText(heightCue(6, 4))).toBeNull();
  });

  it("says the cube is here when the arrow can no longer point well", () => {
    expect(isHere(4, null)).toBe(true);
    expect(isHere(6, 1)).toBe(false);
    expect(isHere(8, 5)).toBe(true); // ±5 m: 8 m away is as good as here
    expect(isHere(9, 30)).toBe(false); // but never further than 8 m
  });

  it("beeps a little faster as you get closer, from 30 m, never frantically", () => {
    expect(beepInterval(31)).toBeNull();
    const far = beepInterval(30)!;
    const mid = beepInterval(15)!;
    const near = beepInterval(2)!;
    expect(far).toBe(1400);
    expect(far).toBeGreaterThan(mid);
    expect(mid).toBeGreaterThan(near);
    expect(near).toBe(400);
  });

  it("beeps only while you head for the cube, then once on arrival", () => {
    const b = new ProximityBeeper();
    const run = (from: number, to: number, dist: (t: number) => number) => {
      const out: string[] = [];
      for (let t = from; t < to; t += 20) {
        const d = dist(t);
        const beep = b.step("c", d, isHere(d, 1), t);
        if (beep) out.push(beep);
      }
      return out;
    };
    // A new cube 25 m away: its alert chime is enough while you stand still.
    expect(run(0, 12_000, () => 25)).toEqual([]);
    // Walking towards it at 5 m/s: beeps all the way, then one "arrived".
    const walk = run(12_000, 16_600, (t) => 25 - ((t - 12_000) / 1000) * 5);
    expect(walk.filter((x) => x === "near").length).toBeGreaterThanOrEqual(4);
    expect(walk.filter((x) => x === "arrived")).toEqual(["arrived"]);
    // Searching right next to it: quiet.
    expect(run(16_600, 40_000, () => 2)).toEqual([]);
    // Walking off, coming halfway back and stopping: quiet 4 s later.
    expect(run(40_000, 43_000, (t) => 2 + ((t - 40_000) / 1000) * 5)).toEqual([]);
    expect(run(43_000, 44_000, (t) => 17 - ((t - 43_000) / 1000) * 5)).toContain("near");
    expect(run(48_100, 56_000, () => 13)).toEqual([]);
    // Coming the rest of the way: "arrived" again.
    const back = run(56_000, 58_400, (t) => 12 - ((t - 56_000) / 1000) * 5);
    expect(back).toContain("near");
    expect(back.filter((x) => x === "arrived")).toEqual(["arrived"]);
    // No cube, or no current position: silent.
    expect(b.step(null, null, false, 60_000)).toBeNull();
    expect(b.step("c", null, false, 60_100)).toBeNull();
  });
});
