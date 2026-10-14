// Mirrors of the Rust types in crates/radar-core (serde camelCase).

export type CubeState = "UNKNOWN" | "SPAWNED" | "DESPAWNED" | "COLLECTED" | "STALE";
export type RotationMode = "north_up" | "heading_up";
export type WindowMode = "overlay" | "separate";
export type StaleDisplay = "hide" | "mark_stale";
export type SourceState = "starting" | "connected" | "waiting" | "finished" | "error" | "disabled";
export type SourceAxis = "pos_x" | "neg_x" | "pos_y" | "neg_y" | "pos_z" | "neg_z";
export type RadarStyle = "radar" | "minimap";

export interface AlertSound {
  enabled: boolean;
  /** 0 – 1. */
  volume: number;
  /** A sound file of your own; the built-in chime when null. */
  file: string | null;
}

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
  /** 1-sigma uncertainty in metres when the position is an estimate. */
  accuracyM: number | null;
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
  | {
      kind: "packet_capture";
      device: string | null;
      file: string | null;
      decoder: "aion2" | "profile" | "none";
      profilePath: string | null;
      north: "neg_y" | "pos_y";
      includeUnconfirmed: boolean;
      record: boolean;
    }
  | { kind: "none" };

export interface CaptureDevice {
  name: string;
  description: string | null;
  ipv4: string[];
  loopback: boolean;
}

export interface OpcodeRow {
  opcode: string;
  count: number;
  minLen: number;
  maxLen: number;
  bundled: number;
  firstMs: number;
  lastMs: number;
  decoded: boolean;
}

export interface Aion2Status {
  clientBuild: string;
  selfId: number | null;
  selfFixAgeMs: number | null;
  mapId: number | null;
  zone: string | null;
  cubesLive: number;
  cubesSeenTotal: number;
  objectsTracked: number;
  lastObjectSpawnMs: number | null;
  learnedTemplates: number[];
  positionSource: "client" | "server" | "estimate" | "none";
  positionAgeMs: number | null;
  movement: { state: "learning" | "found" | "off"; source: unknown; updates: number; clientFrames: number; fixesSeen: number };
  estimate: {
    radiiKnown: number;
    rangeUpdates: number;
    rangeRejected: number;
    checkErrorM: number | null;
    checks: number;
    trusted: boolean;
  };
  warnings: string[];
}

export interface CaptureStatus {
  running: boolean;
  library: string | null;
  devices: string[];
  packets: number;
  streams: {
    segments: number;
    flows: number;
    gameStreams: number;
    gaps: number;
    framing: { frames: number; bundles: number; badBundles: number; resyncs: number; droppedBytes: number };
    currentStream: string | null;
  };
  census: OpcodeRow[];
  frames: number;
  decoder: string | null;
  decoderActive: boolean;
  aion2: Aion2Status | null;
  decoded: number;
  decodeErrors: number;
  recording: string | null;
  recordingBytes: number;
  markersFile: string | null;
  markers: number;
  notes: string[];
  error: string | null;
}

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
  style: RadarStyle;
  mapGrid: boolean;
  showTrail: boolean;
  alertSound: AlertSound;
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
  style: "radar",
  mapGrid: true,
  showTrail: true,
  alertSound: { enabled: true, volume: 0.8, file: null },
};
