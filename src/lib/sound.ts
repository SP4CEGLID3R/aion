// The alert when a new cube appears: the user's own sound file, or a
// built-in chime synthesised here (no sound from any game is bundled).

import { backend } from "./bridge";
import type { AlertSound } from "./types";

let ctx: AudioContext | null = null;
let loaded: { file: string; buffer: AudioBuffer } | null = null;
let loading: Promise<AudioBuffer | null> | null = null;
let lastError: string | null = null;

function audio(): AudioContext {
  ctx ??= new AudioContext();
  // The app's WebView2 allows autoplay (wry's default); resume just in case.
  if (ctx.state === "suspended") void ctx.resume();
  return ctx;
}

async function fileBuffer(file: string): Promise<AudioBuffer | null> {
  if (loaded?.file === file) return loaded.buffer;
  loading ??= (async () => {
    try {
      const bytes = await backend.readSoundFile(file);
      const buffer = await audio().decodeAudioData(bytes.slice(0));
      loaded = { file, buffer };
      lastError = null;
      return buffer;
    } catch (e) {
      lastError = `Could not play ${file}: ${String(e)}`;
      console.error(lastError);
      return null;
    } finally {
      loading = null;
    }
  })();
  return loading;
}

/** A bright, ringing two-note chime, about a second long. */
function chime(ac: AudioContext, volume: number): void {
  const out = ac.createGain();
  out.gain.value = volume;
  out.connect(ac.destination);
  const t0 = ac.currentTime + 0.01;
  const note = (freq: number, at: number, len: number, level: number) => {
    // A bell: a few inharmonic partials, each fading at its own rate.
    for (const [ratio, amp, decay] of [
      [1, 1, 1],
      [2.0, 0.45, 0.7],
      [2.76, 0.3, 0.5],
      [5.4, 0.12, 0.3],
    ] as const) {
      const o = ac.createOscillator();
      const g = ac.createGain();
      o.type = "sine";
      o.frequency.value = freq * ratio;
      g.gain.setValueAtTime(0, at);
      g.gain.linearRampToValueAtTime(level * amp, at + 0.008);
      g.gain.exponentialRampToValueAtTime(0.0001, at + len * decay);
      o.connect(g).connect(out);
      o.start(at);
      o.stop(at + len);
    }
  };
  note(1318.5, t0, 1.1, 0.35); // E6
  note(1975.5, t0 + 0.11, 1.4, 0.3); // B6
}

export async function playAlert(cfg: AlertSound): Promise<void> {
  const ac = audio();
  const volume = Math.min(1, Math.max(0, cfg.volume));
  const buffer = cfg.file ? await fileBuffer(cfg.file) : null;
  if (!buffer) {
    chime(ac, volume);
    return;
  }
  const src = ac.createBufferSource();
  const g = ac.createGain();
  g.gain.value = volume;
  src.buffer = buffer;
  src.connect(g).connect(ac.destination);
  src.start();
}

/** Why the chosen file could not be played, if it could not. */
export function alertError(): string | null {
  return lastError;
}

/**
 * Decides when to sound: once for each cube id newly on the radar, not again
 * for the same id within `REPEAT_MS`, and never more than once per
 * `GAP_MS` (several cubes at once give one alert).
 */
export class AlertGate {
  private seen = new Map<string, number>();
  private zone: string | null = null;
  private last = -Infinity;
  static readonly REPEAT_MS = 5 * 60_000;
  static readonly GAP_MS = 1_500;

  /** True when an alert should play for this frame's live cube ids. */
  check(zone: string | null, liveIds: readonly string[], now: number): boolean {
    if (zone !== this.zone) {
      this.zone = zone;
      this.seen.clear();
    }
    let fresh = false;
    for (const id of liveIds) {
      const at = this.seen.get(id);
      if (at === undefined || now - at > AlertGate.REPEAT_MS) fresh = true;
      this.seen.set(id, now);
    }
    if (!fresh || now - this.last < AlertGate.GAP_MS) return false;
    this.last = now;
    return true;
  }
}
