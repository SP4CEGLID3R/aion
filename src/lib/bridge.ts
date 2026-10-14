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
  type RadarFrame,
  type RadarSettings,
  type SettingsUpdate,
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
