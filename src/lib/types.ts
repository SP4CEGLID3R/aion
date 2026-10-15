// Mirrors of the Rust types in crates/radar-core (serde camelCase).

export type CubeState = "UNKNOWN" | "SPAWNED" | "DESPAWNED" | "COLLECTED" | "STALE";
export type RotationMode = "north_up" | "heading_up";
export type WindowMode = "overlay" | "separate";
export type StaleDisplay = "hide" | "mark_stale";
export type SourceState = "starting" | "connected" | "waiting" | "finished" | "error" | "disabled";
export type SourceAxis = "pos_x" | "neg_x" | "pos_y" | "neg_y" | "pos_z" | "neg_z";
export type RadarStyle = "radar" | "minimap";

/** Physical screen pixels (virtual screen). */
export interface ScreenRegion {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface MinimapSettings {
  enabled: boolean;
  /** null = the default spot of the default HUD. */
  region: ScreenRegion | null;
  fps: number;
  background: boolean;
}

export interface GuideSettings {
  screenArrow: boolean;
  screenX: number;
  screenY: number;
  screenSize: number;
  /** Soft beeps, faster as you get closer to the nearest cube. */
  beep: boolean;
  /** 0 – 1; quiet by default. */
  beepVolume: number;
}

/** The decoder's view of the minimap (scale, state, camera). */
export interface MinimapView {
  pxPerM: number | null;
  calibrated: boolean;
  state: string;
  reason: string;
  cameraDeg: number | null;
  legs: number;
}

/** The screen reader's own status. */
export interface VisionStatus {
  running: boolean;
  region: ScreenRegion | null;
  defaultRegion: boolean;
  uiScale: number;
  frames: number;
  grabMs: number;
  trackMs: number;
  black: boolean;
  frozen: boolean;
  breaks: number;
  coneClear: boolean;
  error: string | null;
}

export interface MonitorInfo {
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  primary: boolean;
}

export interface MinimapFrame {
  seq: number;
  w: number;
  h: number;
  pivotX: number;
  pivotY: number;
  /** Minimap pixels per metre (0 = unknown). */
  pxPerM: number;
  calibrated: boolean;
  rgba: Uint8ClampedArray<ArrayBuffer>;
}

export interface Screenshot {
  w: number;
  h: number;
  x: number;
  y: number;
  rgba: Uint8ClampedArray<ArrayBuffer>;
}

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
  /** 1-sigma uncertainty of the height (metres) when it was not measured with the position; null = measured. */
  heightAccuracyM: number | null;
}

export interface CubeView {
  id: string;
  label: string | null;
  /** Only ever SPAWNED, or STALE when stale cubes are shown. */
  state: CubeState;
  pos: WorldPos;
  distance: number | null;
  bearingDeg: number | null;
  /** Height above the player in metres (negative: below); null without a player position. */
  height: number | null;
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
  positionSource: "minimap" | "client" | "server" | "estimate" | "none";
  positionAgeMs: number | null;
  minimap: {
    state: string;
    reason: string;
    pxPerM: number | null;
    axis: string;
    legs: number;
    axisConfirmed: boolean;
    frames: number;
    breaks: number;
    lastFrameAgeMs: number | null;
    cameraDeg: number | null;
    black: boolean;
    frozen: boolean;
  };
  movement: { state: "learning" | "found" | "off"; source: unknown; updates: number; clientFrames: number; fixesSeen: number };
  estimate: {
    radiiKnown: number;
    rangeUpdates: number;
    rangeRejected: number;
    checkErrorM: number | null;
    checks: number;
    trusted: boolean;
  };
  /** The ground height model behind the own height between fixes. */
  ground: { samples: number; checks: number; missM: number | null; airborne: boolean };
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
  minimap: MinimapSettings;
  guide: GuideSettings;
  /** The radar look the settings were saved with (older files get the clean look once). */
  lookVersion: number;
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
  showLabels: false,
  showDistances: true,
  showIndicators: true,
  showLegend: false,
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
  showTrail: false,
  alertSound: { enabled: true, volume: 0.8, file: null },
  minimap: { enabled: true, region: null, fps: 10, background: true },
  guide: { screenArrow: true, screenX: 0.5, screenY: 0.62, screenSize: 260, beep: true, beepVolume: 0.25 },
  lookVersion: 1,
};
