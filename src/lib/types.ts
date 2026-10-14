// Mirrors of the Rust types in crates/radar-core (serde camelCase).

export type CubeState = "UNKNOWN" | "SPAWNED" | "DESPAWNED" | "COLLECTED" | "STALE";
export type RotationMode = "north_up" | "heading_up";
export type WindowMode = "overlay" | "separate";
export type StaleDisplay = "hide" | "mark_stale";
export type SourceState = "starting" | "connected" | "waiting" | "finished" | "error" | "disabled";
export type SourceAxis = "pos_x" | "neg_x" | "pos_y" | "neg_y" | "pos_z" | "neg_z";

export interface WorldPos {
  x: number;
  y: number;
  z: number;
}

export interface PlayerView {
  pos: WorldPos;
  headingDeg: number | null;
  ageMs: number;
  stale: boolean;
}

export interface CubeView {
  id: string;
  label: string | null;
  /** Only ever SPAWNED, or STALE when stale cubes are shown. */
  state: CubeState;
  pos: WorldPos;
  distance: number | null;
  bearingDeg: number | null;
  confirmedAgeMs: number;
}

export interface FrameStats {
  spawned: number;
  stale: number;
  tracked: number;
  accepted: number;
  duplicates: number;
  rejected: number;
}

export interface SourceHealth {
  name: string;
  state: SourceState;
  detail: string;
  lastEventAt: number | null;
}

export interface RadarFrame {
  generatedAt: number;
  zone: string | null;
  player: PlayerView | null;
  cubes: CubeView[];
  stats: FrameStats;
  source: SourceHealth | null;
}

export interface RadarColors {
  background: string;
  ring: string;
  player: string;
  cube: string;
  stale: string;
  label: string;
  indicator: string;
}

export type DataSourceConfig =
  | { kind: "synthetic"; seed: number; cubeCount: number }
  | { kind: "replay"; path: string; speed: number; loop: boolean }
  | { kind: "local_tcp"; port: number }
  | { kind: "none" };

export interface CoordinateTransform {
  east: SourceAxis;
  north: SourceAxis;
  up: SourceAxis;
  scale: number;
  origin: [number, number, number];
  headingOffsetDeg: number;
  headingClockwise: boolean;
}

export interface RadarSettings {
  range: number;
  rotationMode: RotationMode;
  rotationOffsetDeg: number;
  opacity: number;
  colors: RadarColors;
  showLabels: boolean;
  showDistances: boolean;
  showIndicators: boolean;
  showLegend: boolean;
  staleTimeoutSecs: number;
  staleDisplay: StaleDisplay;
  staleRetainSecs: number;
  windowMode: WindowMode;
  clickThrough: boolean;
  updateHz: number;
  dataSource: DataSourceConfig;
  coordinateTransform: CoordinateTransform;
}

export interface SettingsUpdate {
  settings: RadarSettings;
  warnings: string[];
}

export interface HistoryEntry {
  at: number;
  observedAt: number;
  event: string;
  id: string | null;
  provenance: string | null;
  pos: WorldPos | null;
  outcome: string;
}

export interface CubeRecord {
  id: string;
  state: CubeState;
  pos: WorldPos | null;
  label: string | null;
  zone: string | null;
  lastConfirmedAt: number | null;
  lastEventAt: number;
  stateSince: number;
}

export interface DebugReport {
  generatedAt: number;
  source: SourceHealth;
  zone: string | null;
  diagnostics: {
    accepted: number;
    duplicates: number;
    rejected: Record<string, number>;
    notLiveByProvenance: Record<string, number>;
  };
  adapterErrors: number;
  cubes: CubeRecord[];
  history: HistoryEntry[];
}

export const DEFAULT_SETTINGS: RadarSettings = {
  range: 150,
  rotationMode: "north_up",
  rotationOffsetDeg: 0,
  opacity: 0.85,
  colors: {
    background: "#0b1220",
    ring: "#5b6b85",
    player: "#4fd1ff",
    cube: "#ffcc33",
    stale: "#8a8f98",
    label: "#e8edf5",
    indicator: "#ffcc33",
  },
  showLabels: true,
  showDistances: true,
  showIndicators: true,
  showLegend: true,
  staleTimeoutSecs: 10,
  staleDisplay: "hide",
  staleRetainSecs: 30,
  windowMode: "overlay",
  clickThrough: false,
  updateHz: 20,
  dataSource: { kind: "synthetic", seed: 7, cubeCount: 6 },
  coordinateTransform: {
    east: "pos_x",
    north: "pos_y",
    up: "pos_z",
    scale: 1,
    origin: [0, 0, 0],
    headingOffsetDeg: 0,
    headingClockwise: true,
  },
};
