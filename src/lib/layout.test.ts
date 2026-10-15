import { describe, expect, it } from "vitest";
import { layoutMarkers } from "./layout";
import type { View } from "./projection";
import type { CubeState, CubeView } from "./types";

const view: View = { cx: 100, cy: 100, radiusPx: 100, range: 50, upDeg: 0 };
const all = { showLabels: true, showDistances: true, showIndicators: true };
const origin = { x: 0, y: 0, z: 0 };

function cube(id: string, x: number, y: number, state: CubeState = "SPAWNED", height: number | null = 0): CubeView {
  return {
    id,
    label: `Cube ${id}`,
    state,
    pos: { x, y, z: height ?? 0 },
    distance: null,
    bearingDeg: null,
    height,
    confirmedAgeMs: 0,
  };
}

describe("layoutMarkers", () => {
  it("places a spawned cube in range as a live marker", () => {
    const [m] = layoutMarkers([cube("a", 0, 25)], origin, view, all);
    expect(m).toMatchObject({ id: "a", kind: "live", x: 100, y: 50, text: "Cube a · 25" });
  });

  it("draws stale cubes as stale, and never as indicators", () => {
    const out = layoutMarkers([cube("s", 0, 10, "STALE"), cube("far", 0, 500, "STALE")], origin, view, all);
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ id: "s", kind: "stale" });
    expect(out[0]?.text).toContain("STALE");
  });

  it("drops every state other than SPAWNED and STALE", () => {
    const states: CubeState[] = ["UNKNOWN", "DESPAWNED", "COLLECTED"];
    expect(layoutMarkers(states.map((s, i) => cube(`${i}`, 1, 1, s)), origin, view, all)).toEqual([]);
  });

  it("places nothing without a player position", () => {
    expect(layoutMarkers([cube("a", 0, 25)], null, view, all)).toEqual([]);
  });

  it("turns out-of-range cubes into rim indicators, or hides them", () => {
    const [m] = layoutMarkers([cube("far", 200, 0)], origin, view, all);
    expect(m?.kind).toBe("indicator");
    expect(m?.x).toBeCloseTo(190);
    expect(m?.angle).toBeCloseTo(Math.PI / 2);
    expect(m?.text).toBe("200");
    expect(layoutMarkers([cube("far", 200, 0)], origin, view, { ...all, showIndicators: false })).toEqual([]);
  });

  it("respects label and distance toggles", () => {
    const none = { showLabels: false, showDistances: false, showIndicators: true };
    expect(layoutMarkers([cube("a", 0, 25)], origin, view, none)[0]?.text).toBeNull();
    const onlyDist = { ...none, showDistances: true };
    expect(layoutMarkers([cube("a", 0, 25)], origin, view, onlyDist)[0]?.text).toBe("25");
  });

  it("says up or down when the height is clearly different", () => {
    const [m] = layoutMarkers([cube("hi", 0, 25, "SPAWNED", 12.4)], origin, view, all);
    expect(m?.text).toBe("Cube hi · 25 · ▲12");
    // Your own height is unsure by 8 m: 12 m up is not clear enough.
    expect(layoutMarkers([cube("hi", 0, 25, "SPAWNED", 12.4)], origin, view, all, 8)[0]?.text).toBe("Cube hi · 25");
    const [far] = layoutMarkers([cube("low", 200, 0, "SPAWNED", -9)], origin, view, all, 2);
    expect(far?.text).toBe("200 ▼~9");
    // Never for stale cubes.
    expect(layoutMarkers([cube("s", 0, 10, "STALE", 20)], origin, view, all)[0]?.text).toBe("Cube s · 10 · STALE");
  });

  it("orders the nearest cube last so it draws on top", () => {
    const out = layoutMarkers([cube("near", 0, 5), cube("mid", 0, 20)], origin, view, all);
    expect(out.map((m) => m.id)).toEqual(["mid", "near"]);
  });
});
