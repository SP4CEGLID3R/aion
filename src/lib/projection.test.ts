import { describe, expect, it } from "vitest";
import {
  clampToRim,
  compassPoint,
  formatDistance,
  lerpAngleDeg,
  normalizeDeg,
  projectOffset,
  projectWorld,
  screenAngle,
  viewUpDeg,
  type View,
} from "./projection";

const view = (upDeg = 0): View => ({ cx: 100, cy: 100, radiusPx: 100, range: 50, upDeg });

function near(actual: number, expected: number) {
  expect(actual).toBeCloseTo(expected, 9);
}

describe("projectOffset", () => {
  it("north-up puts north at the top and east on the right", () => {
    const n = projectOffset(0, 25, view());
    near(n.x, 100);
    near(n.y, 50);
    const e = projectOffset(25, 0, view());
    near(e.x, 150);
    near(e.y, 100);
  });

  it("scales by range: the rim is `range` world units away", () => {
    const p = projectOffset(0, -50, view());
    near(p.y, 200);
    near(p.r, 100);
  });

  it("heading-up rotates the heading direction to the top", () => {
    // Facing east: a cube due east is straight ahead (up).
    const ahead = projectOffset(25, 0, view(90));
    near(ahead.x, 100);
    near(ahead.y, 50);
    // And north is now on the left.
    const north = projectOffset(0, 25, view(90));
    near(north.x, 50);
    near(north.y, 100);
  });

  it("is relative to the player", () => {
    const p = projectWorld({ x: 10, y: 10, z: 0 }, { x: 10, y: 35, z: 99 }, view());
    near(p.x, 100);
    near(p.y, 50);
  });
});

describe("viewUpDeg", () => {
  it("north-up ignores the heading", () => {
    expect(viewUpDeg("north_up", 0, 123)).toBe(0);
  });
  it("heading-up follows the heading, plus the offset", () => {
    expect(viewUpDeg("heading_up", 10, 350)).toBe(0);
  });
  it("heading-up without a heading falls back to north-up", () => {
    expect(viewUpDeg("heading_up", 0, null)).toBe(0);
  });
});

describe("rim helpers", () => {
  it("clampToRim keeps the direction of an out-of-range point", () => {
    const far = projectOffset(500, 0, view());
    const c = clampToRim(far, view(), 90);
    near(c.x, 190);
    near(c.y, 100);
    near(c.angle, Math.PI / 2);
  });

  it("screenAngle measures clockwise from the top of the view", () => {
    near(screenAngle(90, view()), Math.PI / 2);
    near(screenAngle(90, view(90)), 0);
    near(screenAngle(0, view(90)), (3 * Math.PI) / 2);
  });
});

describe("angles and formatting", () => {
  it("normalizes degrees", () => {
    expect(normalizeDeg(-90)).toBe(270);
    expect(normalizeDeg(720)).toBe(0);
  });

  it("interpolates angles the short way round", () => {
    near(lerpAngleDeg(350, 10, 0.5), 0);
    near(lerpAngleDeg(10, 350, 0.25), 5);
  });

  it("formats distances and compass points", () => {
    expect(formatDistance(3.14159)).toBe("3.1");
    expect(formatDistance(142.6)).toBe("143");
    expect(formatDistance(Number.NaN)).toBe("?");
    expect(compassPoint(0)).toBe("N");
    expect(compassPoint(44)).toBe("NE");
    expect(compassPoint(359)).toBe("N");
    expect(compassPoint(225)).toBe("SW");
  });
});
