// Decides *what* to draw, separately from drawing it, so the rules can be
// tested without a canvas.
//
// Rules carried over from the Rust core and enforced again here:
//  * Only SPAWNED cubes get a solid marker.
//  * STALE cubes (present in the frame only when the user chose to see them)
//    get a hollow, grey "stale" marker — never the live style.
//  * Any other state is dropped, whatever the frame says.
//  * Without a player position there is no centre, so nothing is placed.

import { heightCue, heightShort } from "./guide";
import { clampToRim, formatDistance, projectWorld, type View } from "./projection";
import type { CubeView, RadarSettings, WorldPos } from "./types";

export type MarkerKind = "live" | "stale" | "indicator";

export interface MarkerLayout {
  id: string;
  kind: MarkerKind;
  x: number;
  y: number;
  /** Screen angle from the centre (radians, 0 = up, clockwise); for indicators. */
  angle: number;
  text: string | null;
  distance: number;
}

/** How far inside the rim indicators sit, in pixels. */
export const INDICATOR_INSET = 10;

export function layoutMarkers(
  cubes: readonly CubeView[],
  player: WorldPos | null,
  view: View,
  settings: Pick<RadarSettings, "showLabels" | "showDistances" | "showIndicators">,
  /** How unsure the player's height is (null = measured), for "▲12 m". */
  heightAccuracyM: number | null = null,
): MarkerLayout[] {
  if (!player) return [];
  const out: MarkerLayout[] = [];
  for (const cube of cubes) {
    if (cube.state !== "SPAWNED" && cube.state !== "STALE") continue;
    const p = projectWorld(player, cube.pos, view);
    const distance = Math.hypot(cube.pos.x - player.x, cube.pos.y - player.y);
    const stale = cube.state === "STALE";
    const inside = p.r <= view.radiusPx - INDICATOR_INSET / 2;
    // Above or below, when clearly so (live cubes only).
    const height = stale ? null : heightShort(heightCue(cube.height, heightAccuracyM));

    const parts: string[] = [];
    if (settings.showLabels) parts.push(cube.label ?? cube.id);
    if (settings.showDistances) parts.push(formatDistance(distance));
    if (settings.showDistances && height) parts.push(height);
    if (stale) parts.push("STALE");
    const text = parts.length ? parts.join(" · ") : null;

    if (inside) {
      out.push({ id: cube.id, kind: stale ? "stale" : "live", x: p.x, y: p.y, angle: 0, text, distance });
    } else if (settings.showIndicators && !stale) {
      // Directional hints are for live cubes only.
      const rim = clampToRim(p, view, view.radiusPx - INDICATOR_INSET);
      const hint = settings.showDistances ? [formatDistance(distance), height].filter(Boolean).join(" ") : null;
      out.push({ id: cube.id, kind: "indicator", x: rim.x, y: rim.y, angle: rim.angle, text: hint, distance });
    }
  }
  // Nearest last, so it is drawn on top.
  return out.sort((a, b) => b.distance - a.distance);
}
