import { describe, expect, it, vi } from "vitest";

vi.mock("./bridge", () => ({ backend: {} }));
const { AlertGate } = await import("./sound");

describe("alert gate", () => {
  it("sounds once per new cube, not for cubes already on the radar", () => {
    const g = new AlertGate();
    expect(g.check("z", [], 0)).toBe(false);
    expect(g.check("z", ["a"], 100)).toBe(true);
    expect(g.check("z", ["a"], 5_000)).toBe(false);
    expect(g.check("z", ["a", "b"], 10_000)).toBe(true);
  });

  it("several cubes at once give one alert", () => {
    const g = new AlertGate();
    expect(g.check("z", ["a"], 0)).toBe(true);
    expect(g.check("z", ["a", "b"], 500)).toBe(false);
    expect(g.check("z", ["a", "b", "c"], 2_000)).toBe(true);
  });

  it("a zone change starts over; a cube back after a long time sounds again", () => {
    const g = new AlertGate();
    expect(g.check("z1", ["a"], 0)).toBe(true);
    expect(g.check("z2", ["a"], 2_000)).toBe(true);
    expect(g.check("z2", ["a"], 4_000)).toBe(false);
    expect(g.check("z2", ["a"], 4_000 + AlertGate.REPEAT_MS + 1)).toBe(true);
  });
});
