/** How long the radar stays after the last cube left it (no flicker when a
 * cube is lost for a frame or two). */
export const LINGER_MS = 1500;

/**
 * "Show the radar only when a cube is on it": whether the radar is visible.
 * It appears at once when a cube is drawn and goes some time after the last
 * one left, or after `reveal` (e.g. to show that the option was switched).
 */
export class AutoHide {
  private lastCubeAt = -Infinity;
  private revealUntil = -Infinity;

  reveal(now: number, ms: number): void {
    this.revealUntil = now + ms;
  }

  visible(enabled: boolean, cubesDrawn: number, now: number): boolean {
    if (cubesDrawn > 0) this.lastCubeAt = now;
    if (!enabled) return true;
    return cubesDrawn > 0 || now - this.lastCubeAt < LINGER_MS || now < this.revealUntil;
  }
}
