// A grid fixed to the world: as the player moves, the lines slide past the
// centre, so the radar reads like a moving map rather than a static disc.

import { projectOffset, type View } from "./projection";
import type { WorldPos } from "./types";

const NICE = [5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000];

/** A round grid spacing that puts about three lines between centre and rim. */
export function gridStep(range: number): number {
  const target = range / 3;
  return NICE.find((s) => s >= target) ?? NICE[NICE.length - 1]!;
}

export interface Segment {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  /** Lines on a multiple of five steps are drawn a little stronger. */
  major: boolean;
}

/** Screen segments of the world grid lines that cross the disc. */
export function gridSegments(player: WorldPos, view: View, step = gridStep(view.range)): Segment[] {
  const out: Segment[] = [];
  // Long enough to cross the disc at any rotation.
  const reach = view.range * 1.5;
  for (const axis of ["east", "north"] as const) {
    const c = axis === "east" ? player.x : player.y;
    const first = Math.ceil((c - reach) / step);
    const last = Math.floor((c + reach) / step);
    for (let k = first; k <= last; k++) {
      const off = k * step - c;
      const [a, b] =
        axis === "east"
          ? [projectOffset(off, -reach, view), projectOffset(off, reach, view)]
          : [projectOffset(-reach, off, view), projectOffset(reach, off, view)];
      out.push({ x1: a.x, y1: a.y, x2: b.x, y2: b.y, major: k % 5 === 0 });
    }
  }
  return out;
}
