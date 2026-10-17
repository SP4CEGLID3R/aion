// The one place the UI talks to the backend. Inside Tauri this is IPC to the
// Rust pipeline; in a plain browser (`pnpm dev`) it is a clearly-labelled
// preview so the UI can be looked at without building the app.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { startPreviewFrames } from "./preview";
import {
  DEFAULT_SETTINGS,
  type CaptureDevice,
  type CaptureStatus,
  type DebugReport,
  type MinimapFrame,
  type MinimapView,
  type MonitorInfo,
  type RadarFrame,
  type RadarSettings,
  type ScreenRegion,
  type Screenshot,
  type SettingsUpdate,
  type VisionStatus,
} from "./types";

export interface Backend {
  readonly isPreview: boolean;
  getSettings(): Promise<RadarSettings>;
  updateSettings(settings: RadarSettings): Promise<SettingsUpdate>;
  resetSettings(): Promise<SettingsUpdate>;
  getDebugReport(): Promise<DebugReport | null>;
  openSettingsWindow(): Promise<void>;
  onFrame(cb: (frame: RadarFrame) => void): Promise<UnlistenFn>;
  onSettings(cb: (settings: RadarSettings) => void): Promise<UnlistenFn>;
  listCaptureDevices(): Promise<CaptureDevice[]>;
  getCaptureStatus(): Promise<CaptureStatus | null>;
  addCaptureMarker(kind: string): Promise<number>;
  getRecordingsDir(): Promise<string>;
  onMarker(cb: (message: string) => void): Promise<UnlistenFn>;
  readSoundFile(path: string): Promise<ArrayBuffer>;
  getMinimapStatus(): Promise<VisionStatus | null>;
  getMinimapView(): Promise<MinimapView | null>;
  /** The latest minimap picture if newer than `after`, else null. */
  minimapFrame(after: number): Promise<MinimapFrame | null>;
  listMonitors(): Promise<MonitorInfo[]>;
  defaultMinimapRegion(index: number): Promise<ScreenRegion>;
  screenshotMonitor(index: number): Promise<Screenshot>;
  /** Puts a shortcut to the app on the desktop; returns its path. */
  createDesktopShortcut(): Promise<string>;
  /** The radar shows nothing right now: let clicks through to the game. */
  setRadarHidden(hidden: boolean): Promise<void>;
}

/** Layout written by `minimap::Shared::frame_after` in the Rust shell. */
export function parseMinimapFrame(buf: ArrayBuffer): MinimapFrame | null {
  if (buf.byteLength < 32) return null;
  const v = new DataView(buf);
  const seq = v.getUint32(0, true) + v.getUint32(4, true) * 2 ** 32;
  const w = v.getUint32(8, true);
  const h = v.getUint32(12, true);
  if (buf.byteLength < 32 + w * h * 4) return null;
  return {
    seq,
    w,
    h,
    pivotX: v.getFloat32(16, true),
    pivotY: v.getFloat32(20, true),
    pxPerM: v.getFloat32(24, true),
    calibrated: v.getUint32(28, true) !== 0,
    rgba: new Uint8ClampedArray(buf, 32, w * h * 4),
  };
}

/** Layout written by `screenshot_monitor`. */
export function parseScreenshot(buf: ArrayBuffer): Screenshot {
  const v = new DataView(buf);
  const w = v.getUint32(0, true);
  const h = v.getUint32(4, true);
  return {
    w,
    h,
    x: v.getInt32(8, true),
    y: v.getInt32(12, true),
    rgba: new Uint8ClampedArray(buf, 16, w * h * 4),
  };
}

export const FRAME_EVENT = "radar-frame";
export const SETTINGS_EVENT = "settings-changed";
export const MARKER_EVENT = "capture-marker";

function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

const tauriBackend: Backend = {
  isPreview: false,
  getSettings: () => invoke<RadarSettings>("get_settings"),
  updateSettings: (settings) => invoke<SettingsUpdate>("update_settings", { settings }),
  resetSettings: () => invoke<SettingsUpdate>("reset_settings"),
  getDebugReport: () => invoke<DebugReport>("get_debug_report"),
  openSettingsWindow: () => invoke<void>("open_settings_window"),
  onFrame: (cb) => listen<RadarFrame>(FRAME_EVENT, (e) => cb(e.payload)),
  onSettings: (cb) => listen<RadarSettings>(SETTINGS_EVENT, (e) => cb(e.payload)),
  listCaptureDevices: () => invoke<CaptureDevice[]>("list_capture_devices"),
  getCaptureStatus: () => invoke<CaptureStatus | null>("get_capture_status"),
  addCaptureMarker: (kind) => invoke<number>("add_capture_marker", { kind }),
  getRecordingsDir: () => invoke<string>("get_recordings_dir"),
  onMarker: (cb) => listen<string>(MARKER_EVENT, (e) => cb(e.payload)),
  readSoundFile: (path) => invoke<ArrayBuffer>("read_sound_file", { path }),
  getMinimapStatus: () => invoke<VisionStatus | null>("get_minimap_status"),
  getMinimapView: () => invoke<MinimapView | null>("get_minimap_view"),
  minimapFrame: async (after) => parseMinimapFrame(await invoke<ArrayBuffer>("minimap_frame", { after })),
  listMonitors: () => invoke<MonitorInfo[]>("list_monitors"),
  defaultMinimapRegion: (index) => invoke<ScreenRegion>("default_minimap_region", { index }),
  screenshotMonitor: async (index) => parseScreenshot(await invoke<ArrayBuffer>("screenshot_monitor", { index })),
  createDesktopShortcut: () => invoke<string>("create_desktop_shortcut"),
  setRadarHidden: (hidden) => invoke<void>("set_radar_hidden", { hidden }),
};

const PREVIEW_KEY = "aion2-radar-preview-settings";

function previewBackend(): Backend {
  const listeners = new Set<(s: RadarSettings) => void>();
  const load = (): RadarSettings => {
    try {
      const raw = localStorage.getItem(PREVIEW_KEY);
      return raw ? { ...DEFAULT_SETTINGS, ...(JSON.parse(raw) as Partial<RadarSettings>) } : DEFAULT_SETTINGS;
    } catch {
      return DEFAULT_SETTINGS;
    }
  };
  const save = (s: RadarSettings): SettingsUpdate => {
    try {
      localStorage.setItem(PREVIEW_KEY, JSON.stringify(s));
    } catch {
      // Storage can be unavailable; the preview still works for this page.
    }
    listeners.forEach((cb) => cb(s));
    return { settings: s, warnings: ["Browser preview: settings are not validated by the Rust core."] };
  };
  return {
    isPreview: true,
    getSettings: async () => load(),
    updateSettings: async (s) => save(s),
    resetSettings: async () => save(DEFAULT_SETTINGS),
    getDebugReport: async () => null,
    openSettingsWindow: async () => {
      window.open("settings.html", "_blank");
    },
    onFrame: async (cb) => startPreviewFrames(cb, load),
    listCaptureDevices: async () => [],
    getCaptureStatus: async () => null,
    addCaptureMarker: async () => {
      throw new Error("Packet capture runs only in the desktop app.");
    },
    getRecordingsDir: async () => "(desktop app only)",
    onMarker: async () => () => {},
    readSoundFile: async () => {
      throw new Error("Sound files can be loaded only in the desktop app.");
    },
    getMinimapStatus: async () => null,
    getMinimapView: async () => null,
    minimapFrame: async () => null,
    listMonitors: async () => [],
    defaultMinimapRegion: async () => {
      throw new Error("Screen capture runs only in the desktop app.");
    },
    screenshotMonitor: async () => {
      throw new Error("Screen capture runs only in the desktop app.");
    },
    createDesktopShortcut: async () => {
      throw new Error("Shortcuts can be made only in the desktop app.");
    },
    setRadarHidden: async () => {},
    onSettings: async (cb) => {
      listeners.add(cb);
      const onStorage = (e: StorageEvent) => e.key === PREVIEW_KEY && cb(load());
      window.addEventListener("storage", onStorage);
      return () => {
        listeners.delete(cb);
        window.removeEventListener("storage", onStorage);
      };
    },
  };
}

export const backend: Backend = inTauri() ? tauriBackend : previewBackend();
