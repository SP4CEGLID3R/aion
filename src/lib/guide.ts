// Which cube the guide arrows point at, and where to draw them.

import { normalizeDeg } from "./projection";
import type { CubeView } from "./types";

/** The nearest live (SPAWNED) cube with a known bearing, or null. */
export function nearestLive(cubes: readonly CubeView[]): CubeView | null {
  let best: CubeView | null = null;
  for (const c of cubes) {
    if (c.state !== "SPAWNED" || c.distance === null || c.bearingDeg === null) continue;
    if (best === null || c.distance < (best.distance ?? Infinity)) best = c;
  }
  return best;
}

/** Bearing relative to where the camera looks: 0 = straight ahead, 90 = right. */
export function relativeBearing(bearingDeg: number, cameraDeg: number): number {
  return normalizeDeg(bearingDeg - cameraDeg);
}

/**
 * A point on the ground ring around the character, seen from behind and
 * above: a flattened ellipse, "ahead" at the top. Returns the point and the
 * on-screen direction of the arrow (radians, 0 = up, clockwise).
 */
export function ringPoint(
  relDeg: number,
  cx: number,
  cy: number,
  rx: number,
  ry: number,
): { x: number; y: number; angle: number } {
  const a = (relDeg * Math.PI) / 180;
  const x = cx + rx * Math.sin(a);
  const y = cy - ry * Math.cos(a);
  // Direction of the ground vector after the same flattening.
  const angle = Math.atan2(rx * Math.sin(a), ry * Math.cos(a));
  return { x, y, angle };
}

export function formatMetres(d: number): string {
  return d < 10 ? `${d.toFixed(1)} m` : `${Math.round(d)} m`;
}
