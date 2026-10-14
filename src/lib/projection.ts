// World → screen projection for the radar canvas.
//
// World frame (from the Rust core): x = east, y = north, z = up.
// Screen frame: origin at the radar centre, x right, y DOWN, in CSS pixels.
// Angles are compass degrees: 0 = north, 90 = east, clockwise.

import type { RotationMode, WorldPos } from "./types";

export interface View {
  cx: number;
  cy: number;
  /** Radius of the radar disc in pixels. */
  radiusPx: number;
  /** World units from centre to rim. */
  range: number;
  /** Compass direction drawn at the top of the radar. */
  upDeg: number;
}

export interface ScreenPoint {
  x: number;
  y: number;
  /** Distance from the centre in pixels. */
  r: number;
}

export const DEG = Math.PI / 180;

export function normalizeDeg(deg: number): number {
  const r = ((deg % 360) + 360) % 360;
  return r >= 360 ? 0 : r;
}

/** Which compass direction points up, given the rotation settings. */
export function viewUpDeg(mode: RotationMode, offsetDeg: number, headingDeg: number | null): number {
  const base = mode === "heading_up" && headingDeg !== null ? headingDeg : 0;
  return normalizeDeg(base + offsetDeg);
}

/** Projects an east/north offset (world units) to screen pixels. */
export function projectOffset(east: number, north: number, view: View): ScreenPoint {
  const ppu = view.radiusPx / view.range;
  // Rotate the world by -upDeg so that compass `upDeg` lands at the top.
  const a = view.upDeg * DEG;
  const cos = Math.cos(a);
  const sin = Math.sin(a);
  const rx = east * cos - north * sin;
  const ry = east * sin + north * cos;
  const x = rx * ppu;
  const y = -ry * ppu;
  return { x: view.cx + x, y: view.cy + y, r: Math.hypot(x, y) };
}

export function projectWorld(player: WorldPos, target: WorldPos, view: View): ScreenPoint {
  return projectOffset(target.x - player.x, target.y - player.y, view);
}

/** Screen angle (radians, 0 = up, clockwise) of a compass direction. */
export function screenAngle(compassDeg: number, view: View): number {
  return normalizeDeg(compassDeg - view.upDeg) * DEG;
}

/** A point on a circle of radius `r` around the centre, at a screen angle. */
export function pointAt(angleRad: number, r: number, view: View): { x: number; y: number } {
  return { x: view.cx + r * Math.sin(angleRad), y: view.cy - r * Math.cos(angleRad) };
}

/** Pulls a point outside the disc back onto a circle of `rimPx`, keeping its direction. */
export function clampToRim(p: ScreenPoint, view: View, rimPx: number): { x: number; y: number; angle: number } {
  const dx = p.x - view.cx;
  const dy = p.y - view.cy;
  const angle = Math.atan2(dx, -dy);
  return { ...pointAt(angle, rimPx, view), angle };
}

export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

/** Interpolates angles along the shorter arc. */
export function lerpAngleDeg(a: number, b: number, t: number): number {
  const d = ((b - a + 540) % 360) - 180;
  return normalizeDeg(a + d * t);
}

export function formatDistance(d: number): string {
  if (!Number.isFinite(d)) return "?";
  return d < 10 ? d.toFixed(1) : Math.round(d).toString();
}

const COMPASS = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"] as const;

export function compassPoint(deg: number): string {
  return COMPASS[Math.round(normalizeDeg(deg) / 45) % 8] ?? "N";
}
