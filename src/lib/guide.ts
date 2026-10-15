// Which cube the guide arrows point at, where to draw them, and what to say:
// the distance, whether the cube is above or below you, "it is here" when
// you are on top of it, and how fast the proximity beeps go.

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

// ---------------------------------------------------------------------------
// Above or below
// ---------------------------------------------------------------------------

/** Closer in height than this counts as "your height". */
export const LEVEL_M = 3;

export type HeightCue =
  | { kind: "up" | "down"; metres: number; approx: boolean }
  | { kind: "level" }
  | { kind: "unknown" };

/**
 * Whether the cube is above or below you. `height`: the cube's height above
 * you (metres, negative = below); `heightAccuracyM`: how unsure your own
 * height is (1 sigma; null = measured). Up or down is only said when the
 * difference is clearly bigger than that doubt.
 */
export function heightCue(height: number | null, heightAccuracyM: number | null): HeightCue {
  if (height === null || !Number.isFinite(height)) return { kind: "unknown" };
  const doubt = heightAccuracyM !== null && Number.isFinite(heightAccuracyM) ? Math.max(0, heightAccuracyM) : 0;
  const a = Math.abs(height);
  if (a >= LEVEL_M && a >= 2 * doubt) {
    return { kind: height > 0 ? "up" : "down", metres: Math.round(a), approx: doubt > 1.5 };
  }
  if (a < LEVEL_M && a + 2 * doubt <= LEVEL_M + 1) return { kind: "level" };
  return { kind: "unknown" };
}

/** Full words, for the arrow over the game: "▲ 12 m up", "same height". */
export function heightText(cue: HeightCue): string | null {
  switch (cue.kind) {
    case "up":
      return `▲ ${cue.approx ? "~" : ""}${cue.metres} m up`;
    case "down":
      return `▼ ${cue.approx ? "~" : ""}${cue.metres} m down`;
    case "level":
      return "same height";
    case "unknown":
      return null;
  }
}

// ---------------------------------------------------------------------------
// On top of it
// ---------------------------------------------------------------------------

/** Within this distance the cube is "here": look around rather than follow the arrow. */
export const HERE_M = 4;

/**
 * You are on top of the cube: closer than the arrow can usefully point
 * (it needs your position to be a good deal better than the distance).
 */
export function isHere(distance: number, accuracyM: number | null): boolean {
  const doubt = accuracyM !== null && Number.isFinite(accuracyM) ? accuracyM : 0;
  return distance <= Math.min(8, Math.max(HERE_M, 2 * doubt));
}

/** Within this distance an unknown height is worth a hint to measure it. */
export const HEIGHT_HINT_M = 30;

// ---------------------------------------------------------------------------
// Proximity beeps
// ---------------------------------------------------------------------------

/** Beeps start this close. */
export const BEEP_FROM_M = 30;
/** Slowest and fastest gap between beeps: never a frantic rattle. */
const BEEP_SLOW_MS = 1400;
const BEEP_FAST_MS = 400;
/** Beeps stop this soon after you stop getting closer. */
const BEEP_KEEP_MS = 4_000;
/** Getting closer by this much counts as heading for the cube. */
const BEEP_PROGRESS_M = 1.5;
/** No beeps right after a new cube appears (its alert chime plays then). */
const NEW_CUBE_QUIET_MS = 1_500;

const clamp01 = (x: number) => Math.min(1, Math.max(0, x));

/** Time between beeps at this distance; null = silent (too far). */
export function beepInterval(distance: number): number | null {
  if (!Number.isFinite(distance) || distance > BEEP_FROM_M) return null;
  const f = clamp01((distance - HERE_M) / (BEEP_FROM_M - HERE_M));
  return BEEP_FAST_MS + f * (BEEP_SLOW_MS - BEEP_FAST_MS);
}

/** Beep pitch: a little higher as you get closer, never shrill. */
export function beepPitch(distance: number): number {
  return 880 - clamp01(distance / BEEP_FROM_M) * 220;
}

/** "near": one soft beep; "arrived": the you-are-there chime. */
export type Beep = "near" | "arrived";

/**
 * Decides when to beep: softly, only while you are heading for the nearest
 * cube, a little faster as you get closer; once on top of it, one "arrived"
 * chime and then quiet (the green circle takes over). It goes quiet a few
 * seconds after you stop going for the cube, so it never nags.
 */
export class ProximityBeeper {
  private target: string | null = null;
  /** Distance when you last got closer (or the furthest since). */
  private ref = Infinity;
  private progressAt = -Infinity;
  private lastBeep = -Infinity;
  private quietUntil = -Infinity;
  /** The "arrived" chime played for this cube (until you walk off again). */
  private arrived = false;

  /**
   * `distance`: to the nearest live cube, null when there is none or your
   * position is not current; `here`: you are on top of it (see isHere).
   */
  step(targetId: string | null, distance: number | null, here: boolean, now: number): Beep | null {
    if (targetId === null || distance === null || !Number.isFinite(distance)) {
      this.target = null;
      return null;
    }
    if (targetId !== this.target) {
      // A new cube: its alert chime says so; beeps follow once you go for it.
      this.target = targetId;
      this.ref = distance;
      this.progressAt = -Infinity;
      this.quietUntil = now + NEW_CUBE_QUIET_MS;
      this.arrived = false;
    } else if (distance < this.ref - BEEP_PROGRESS_M) {
      this.ref = distance;
      this.progressAt = now;
    } else if (distance > this.ref) {
      // Moving away: coming back counts as heading for it again.
      this.ref = distance;
    }
    if (now < this.quietUntil) return null;
    if (here) {
      if (this.arrived) return null;
      this.arrived = true;
      return "arrived";
    }
    if (distance > 2 * HERE_M) this.arrived = false;
    if (now - this.progressAt > BEEP_KEEP_MS) return null;
    const interval = beepInterval(distance);
    if (interval === null || now - this.lastBeep < interval) return null;
    this.lastBeep = now;
    return "near";
  }
}
