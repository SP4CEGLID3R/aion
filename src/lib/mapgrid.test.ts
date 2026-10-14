import { describe, expect, it } from "vitest";
import { gridSegments, gridStep } from "./mapgrid";
import type { View } from "./projection";

const view: View = { cx: 100, cy: 100, radiusPx: 100, range: 150, upDeg: 0 };

describe("world grid", () => {
  it("picks round spacings", () => {
    expect(gridStep(150)).toBe(50);
    expect(gridStep(100)).toBe(50);
    expect(gridStep(60)).toBe(20);
    expect(gridStep(1000)).toBe(500);
  });

  it("is fixed to the world: moving east slides the lines west on screen", () => {
    const at = (x: number) =>
      gridSegments({ x, y: 0, z: 0 }, view)
        .filter((s) => s.x1 === s.x2) // north-south lines in north-up
        .map((s) => s.x1)
        .sort((a, b) => a - b);
    const before = at(0);
    const after = at(10); // 10 m east = 6.67 px at this scale
    expect(before).toContain(100);
    expect(after).toContain(100 - (10 * 100) / 150);
  });

  it("covers the whole disc", () => {
    const segs = gridSegments({ x: 12, y: -7, z: 0 }, view);
    // Lines every 50 m over ±225 m on each axis.
    expect(segs.length).toBeGreaterThanOrEqual(16);
  });
});
