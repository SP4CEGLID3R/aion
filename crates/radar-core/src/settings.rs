//! User-facing settings and their validation.
//!
//! Settings are loaded from disk and edited from the UI, so every field is
//! clamped into a safe range by [`RadarSettings::sanitized`] rather than
//! trusted. The returned warnings say what was corrected.

use serde::{Deserialize, Serialize};

use crate::frame::StaleDisplay;
use crate::geometry::CoordinateTransform;
use crate::tracker::TrackerConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RotationMode {
    /// North is always up.
    #[default]
    NorthUp,
    /// The player's heading is up (falls back to north-up without a heading).
    HeadingUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    /// Transparent, borderless, always on top.
    #[default]
    Overlay,
    /// A normal framed window that does not stay on top.
    Separate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RadarStyle {
    /// A disc with range rings, compass and a moving map grid.
    #[default]
    Radar,
    /// Markers only, on a fully transparent background: for placing the
    /// window over the game's own minimap at the same scale.
    Minimap,
}

/// A rectangle in physical screen pixels (the virtual screen: may be
/// negative left of or above the primary monitor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Following the player with the in-game minimap (read from the screen).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MinimapSettings {
    pub enabled: bool,
    /// The minimap's map area (below its title bar). `None`: the default
    /// spot of the default HUD, top right of the primary monitor.
    pub region: Option<ScreenRegion>,
    /// Captures per second.
    pub fps: u32,
    /// Draw the live minimap behind the radar.
    pub background: bool,
}

impl Default for MinimapSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            region: None,
            fps: 10,
            background: true,
        }
    }
}

pub const MINIMAP_FPS_LIMITS: (u32, u32) = (4, 20);

/// Arrows that point at the nearest live cube.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GuideSettings {
    /// Over the game, around your character (needs the camera direction
    /// from minimap tracking).
    pub screen_arrow: bool,
    /// Where your character is on the screen, as fractions of the monitor.
    pub screen_x: f64,
    pub screen_y: f64,
    /// Size of the on-screen arrow ring, physical pixels.
    pub screen_size: f64,
    /// Soft beeps, faster as you get closer to the nearest cube.
    pub beep: bool,
    /// 0 – 1; quiet by default.
    pub beep_volume: f64,
}

impl Default for GuideSettings {
    fn default() -> Self {
        Self {
            screen_arrow: true,
            screen_x: 0.5,
            screen_y: 0.62,
            screen_size: 260.0,
            beep: true,
            beep_volume: 0.25,
        }
    }
}

/// A sound when a new cube appears on the radar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AlertSound {
    pub enabled: bool,
    /// 0 – 1.
    pub volume: f64,
    /// A sound file of your own (.wav, .mp3, .ogg, .flac); the built-in
    /// chime when empty.
    pub file: Option<String>,
}

impl Default for AlertSound {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 0.8,
            file: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RadarColors {
    pub background: String,
    pub ring: String,
    pub player: String,
    pub cube: String,
    pub stale: String,
    pub label: String,
    pub indicator: String,
}

impl Default for RadarColors {
    fn default() -> Self {
        Self {
            background: "#0b1220".into(),
            ring: "#5b6b85".into(),
            player: "#4fd1ff".into(),
            cube: "#ffcc33".into(),
            stale: "#8a8f98".into(),
            label: "#e8edf5".into(),
            indicator: "#ffcc33".into(),
        }
    }
}

/// Where radar events come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DataSourceConfig {
    /// Deterministic simulated world. Clearly labelled as synthetic.
    Synthetic {
        #[serde(default = "default_seed")]
        seed: u64,
        #[serde(default = "default_cube_count", rename = "cubeCount")]
        cube_count: u32,
    },
    /// Replays a JSON-lines (`.jsonl`) or synthetic-frame (`.srf`, `.srf.hex`) file.
    Replay {
        path: String,
        #[serde(default = "default_speed")]
        speed: f64,
        #[serde(default, rename = "loop")]
        looped: bool,
    },
    /// Accepts Radar Event Protocol v1 JSON lines on 127.0.0.1 from a local,
    /// authorized telemetry producer.
    LocalTcp {
        #[serde(default = "default_port")]
        port: u16,
    },
    /// Passive capture of the game's own traffic (Npcap/libpcap), decoded
    /// with a decoder profile. Built by the app from `radar-capture`.
    PacketCapture {
        /// Adapter name; `None` captures on every adapter and finds the game
        /// connection by its heartbeat.
        #[serde(default)]
        device: Option<String>,
        /// Replay a `.pcap`/`.pcapng` instead of capturing live.
        #[serde(default)]
        file: Option<String>,
        /// What turns game packets into cubes.
        #[serde(default)]
        decoder: CaptureDecoder,
        /// Decoder profile JSON (decoder = profile). Unvalidated draws nothing.
        #[serde(default, rename = "profilePath")]
        profile_path: Option<String>,
        /// Which game axis is map north (built-in decoder).
        #[serde(default)]
        north: CaptureNorth,
        /// Also treat cube types known only from a single unverified source
        /// (e.g. Altgard) as cubes. Their markers say so.
        #[serde(default, rename = "includeUnconfirmed")]
        include_unconfirmed: bool,
        /// Record the game connection (and markers) for discovery work.
        #[serde(default)]
        record: bool,
    },
    /// No source: the radar shows only its "no data" state.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDecoder {
    /// Built-in AION 2 hidden-cube decoder.
    #[default]
    Aion2,
    /// A user-written decoder profile.
    Profile,
    /// Census and recording only.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureNorth {
    /// Map north = world −Y (community finding, unconfirmed).
    #[default]
    NegY,
    /// Map north = world +Y.
    PosY,
}

fn default_seed() -> u64 {
    7
}
fn default_cube_count() -> u32 {
    6
}
fn default_speed() -> f64 {
    1.0
}
fn default_port() -> u16 {
    47_810
}

impl Default for DataSourceConfig {
    fn default() -> Self {
        Self::Synthetic {
            seed: default_seed(),
            cube_count: default_cube_count(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RadarSettings {
    /// World units from the centre to the edge of the radar.
    pub range: f64,
    pub rotation_mode: RotationMode,
    /// Extra rotation applied on top of the mode, in degrees clockwise.
    pub rotation_offset_deg: f64,
    /// Opacity of the whole radar, 0.1 – 1.
    pub opacity: f64,
    pub colors: RadarColors,
    pub show_labels: bool,
    pub show_distances: bool,
    /// Arrows on the rim for cubes beyond the range.
    pub show_indicators: bool,
    pub show_legend: bool,
    /// Seconds without confirmation before a cube (or the player) is stale.
    pub stale_timeout_secs: f64,
    pub stale_display: StaleDisplay,
    /// Seconds a stale cube is kept before it is forgotten.
    pub stale_retain_secs: f64,
    pub window_mode: WindowMode,
    /// Overlay mode only: mouse clicks pass through to the window below.
    pub click_through: bool,
    /// Frames pushed to the renderer per second.
    pub update_hz: u32,
    pub data_source: DataSourceConfig,
    pub coordinate_transform: CoordinateTransform,
    pub style: RadarStyle,
    /// A grid fixed to the world, so the map visibly moves as you do.
    pub map_grid: bool,
    /// Your path over the last minute.
    pub show_trail: bool,
    pub alert_sound: AlertSound,
    pub minimap: MinimapSettings,
    pub guide: GuideSettings,
    /// Which version of the radar's look these settings were saved with;
    /// older files get the current clean look once (see `sanitized`).
    #[serde(default)]
    pub look_version: u32,
}

/// The radar's look: 1 = like a minimap with cubes (distances only, no
/// names, trail or legend unless asked for).
pub const LOOK_VERSION: u32 = 1;

impl Default for RadarSettings {
    fn default() -> Self {
        Self {
            range: 150.0,
            rotation_mode: RotationMode::NorthUp,
            rotation_offset_deg: 0.0,
            opacity: 0.85,
            colors: RadarColors::default(),
            show_labels: false,
            show_distances: true,
            show_indicators: true,
            show_legend: false,
            stale_timeout_secs: 10.0,
            stale_display: StaleDisplay::Hide,
            stale_retain_secs: 30.0,
            window_mode: WindowMode::Overlay,
            click_through: false,
            update_hz: 20,
            data_source: DataSourceConfig::default(),
            coordinate_transform: CoordinateTransform::default(),
            style: RadarStyle::Radar,
            map_grid: true,
            show_trail: false,
            alert_sound: AlertSound::default(),
            minimap: MinimapSettings::default(),
            guide: GuideSettings::default(),
            look_version: LOOK_VERSION,
        }
    }
}

pub const RANGE_LIMITS: (f64, f64) = (5.0, 10_000.0);
pub const OPACITY_LIMITS: (f64, f64) = (0.1, 1.0);
pub const STALE_TIMEOUT_LIMITS: (f64, f64) = (1.0, 600.0);
pub const STALE_RETAIN_LIMITS: (f64, f64) = (0.0, 3_600.0);
pub const UPDATE_HZ_LIMITS: (u32, u32) = (1, 60);

fn clamp_f(
    name: &str,
    v: f64,
    (lo, hi): (f64, f64),
    default: f64,
    warnings: &mut Vec<String>,
) -> f64 {
    if !v.is_finite() {
        warnings.push(format!("{name} was not a number; reset to {default}"));
        return default;
    }
    if v < lo || v > hi {
        let c = v.clamp(lo, hi);
        warnings.push(format!("{name} {v} is outside {lo}–{hi}; using {c}"));
        return c;
    }
    v
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`.
pub fn is_hex_color(s: &str) -> bool {
    let Some(hex) = s.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

impl RadarSettings {
    /// Returns a copy with every field in range, plus what was corrected.
    pub fn sanitized(&self) -> (RadarSettings, Vec<String>) {
        let d = RadarSettings::default();
        let mut w = Vec::new();
        let mut s = self.clone();

        // Saved before the clean look: switch to it once; later choices stay.
        if s.look_version < LOOK_VERSION {
            s.show_labels = d.show_labels;
            s.show_legend = d.show_legend;
            s.show_trail = d.show_trail;
            s.look_version = LOOK_VERSION;
        }

        s.range = clamp_f("range", s.range, RANGE_LIMITS, d.range, &mut w);
        s.opacity = clamp_f("opacity", s.opacity, OPACITY_LIMITS, d.opacity, &mut w);
        s.stale_timeout_secs = clamp_f(
            "staleTimeoutSecs",
            s.stale_timeout_secs,
            STALE_TIMEOUT_LIMITS,
            d.stale_timeout_secs,
            &mut w,
        );
        s.stale_retain_secs = clamp_f(
            "staleRetainSecs",
            s.stale_retain_secs,
            STALE_RETAIN_LIMITS,
            d.stale_retain_secs,
            &mut w,
        );
        if !s.rotation_offset_deg.is_finite() {
            w.push("rotationOffsetDeg was not a number; reset to 0".into());
            s.rotation_offset_deg = 0.0;
        } else {
            s.rotation_offset_deg = crate::geometry::normalize_deg(s.rotation_offset_deg);
        }
        let (lo, hi) = UPDATE_HZ_LIMITS;
        if !(lo..=hi).contains(&s.update_hz) {
            let c = s.update_hz.clamp(lo, hi);
            w.push(format!(
                "updateHz {} is outside {lo}–{hi}; using {c}",
                s.update_hz
            ));
            s.update_hz = c;
        }

        let dc = RadarColors::default();
        let fix = |name: &str, v: &mut String, def: &str, w: &mut Vec<String>| {
            if !is_hex_color(v) {
                w.push(format!(
                    "colors.{name} {v:?} is not a hex colour; using {def}"
                ));
                *v = def.to_owned();
            }
        };
        fix(
            "background",
            &mut s.colors.background,
            &dc.background,
            &mut w,
        );
        fix("ring", &mut s.colors.ring, &dc.ring, &mut w);
        fix("player", &mut s.colors.player, &dc.player, &mut w);
        fix("cube", &mut s.colors.cube, &dc.cube, &mut w);
        fix("stale", &mut s.colors.stale, &dc.stale, &mut w);
        fix("label", &mut s.colors.label, &dc.label, &mut w);
        fix("indicator", &mut s.colors.indicator, &dc.indicator, &mut w);

        let a = &mut s.alert_sound;
        if !(a.volume.is_finite() && (0.0..=1.0).contains(&a.volume)) {
            let c = if a.volume.is_finite() {
                a.volume.clamp(0.0, 1.0)
            } else {
                0.8
            };
            w.push(format!(
                "alertSound.volume {} is outside 0–1; using {c}",
                a.volume
            ));
            a.volume = c;
        }
        if a.file.as_deref().is_some_and(|f| f.trim().is_empty()) {
            a.file = None;
        }

        let m = &mut s.minimap;
        let (lo, hi) = MINIMAP_FPS_LIMITS;
        if !(lo..=hi).contains(&m.fps) {
            let c = m.fps.clamp(lo, hi);
            w.push(format!(
                "minimap.fps {} is outside {lo}–{hi}; using {c}",
                m.fps
            ));
            m.fps = c;
        }
        if let Some(r) = m.region {
            if !(40..=2000).contains(&r.width) || !(40..=2000).contains(&r.height) {
                w.push(format!(
                    "minimap region {}×{} is not a plausible minimap; using the default spot",
                    r.width, r.height
                ));
                m.region = None;
            }
        }
        let g = &mut s.guide;
        for (name, v, def) in [
            ("guide.screenX", &mut g.screen_x, 0.5),
            ("guide.screenY", &mut g.screen_y, 0.62),
            ("guide.beepVolume", &mut g.beep_volume, 0.25),
        ] {
            if !(v.is_finite() && (0.0..=1.0).contains(v)) {
                w.push(format!("{name} must be between 0 and 1; using {def}"));
                *v = def;
            }
        }
        if !(g.screen_size.is_finite() && (80.0..=1200.0).contains(&g.screen_size)) {
            w.push("guide.screenSize must be 80–1200 px; using 260".into());
            g.screen_size = 260.0;
        }

        if let Err(e) = s.coordinate_transform.validate() {
            w.push(format!("coordinateTransform invalid ({e}); using identity"));
            s.coordinate_transform = CoordinateTransform::default();
        }

        match &mut s.data_source {
            DataSourceConfig::Replay { path, speed, .. } => {
                if !(speed.is_finite() && *speed > 0.0) {
                    w.push(format!("replay speed {speed} is invalid; using 1"));
                    *speed = 1.0;
                } else if *speed > 100.0 {
                    w.push(format!("replay speed {speed} is above 100; using 100"));
                    *speed = 100.0;
                }
                if path.trim().is_empty() {
                    w.push("replay path is empty; data source disabled".into());
                    s.data_source = DataSourceConfig::None;
                }
            }
            DataSourceConfig::LocalTcp { port } if *port < 1024 => {
                w.push(format!(
                    "port {port} is privileged; using {}",
                    default_port()
                ));
                *port = default_port();
            }
            DataSourceConfig::PacketCapture {
                device,
                file,
                profile_path,
                ..
            } => {
                for v in [device, file, profile_path] {
                    if v.as_deref().is_some_and(|x| x.trim().is_empty()) {
                        *v = None;
                    }
                }
            }
            DataSourceConfig::Synthetic { cube_count, .. } if *cube_count > 200 => {
                w.push(format!("cubeCount {cube_count} is above 200; using 200"));
                *cube_count = 200;
            }
            _ => {}
        }

        (s, w)
    }

    pub fn tracker_config(&self) -> TrackerConfig {
        TrackerConfig {
            stale_timeout_ms: (self.stale_timeout_secs * 1000.0).round() as i64,
            stale_retain_ms: (self.stale_retain_secs * 1000.0).round() as i64,
            ..TrackerConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_already_sanitized() {
        let (s, w) = RadarSettings::default().sanitized();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s, RadarSettings::default());
    }

    #[test]
    fn out_of_range_values_are_clamped_with_warnings() {
        let s = RadarSettings {
            range: 0.0,
            opacity: 7.0,
            stale_timeout_secs: f64::NAN,
            rotation_offset_deg: -90.0,
            update_hz: 500,
            ..Default::default()
        };
        let (s, w) = s.sanitized();
        assert_eq!(s.range, RANGE_LIMITS.0);
        assert_eq!(s.opacity, 1.0);
        assert_eq!(s.stale_timeout_secs, 10.0);
        assert_eq!(s.rotation_offset_deg, 270.0);
        assert_eq!(s.update_hz, 60);
        assert_eq!(w.len(), 4, "{w:?}"); // rotation is normalised silently
    }

    #[test]
    fn older_settings_get_the_clean_look_once() {
        let old: RadarSettings = serde_json::from_str(
            r#"{"showLabels":true,"showLegend":true,"showTrail":true,"range":80}"#,
        )
        .unwrap();
        assert_eq!(old.look_version, 0);
        let (s, w) = old.sanitized();
        assert!(w.is_empty(), "{w:?}");
        assert!(!s.show_labels && !s.show_legend && !s.show_trail);
        assert_eq!(s.range, 80.0);
        assert_eq!(s.look_version, LOOK_VERSION);
        // Turned back on afterwards: kept.
        let again = RadarSettings {
            show_labels: true,
            ..s
        };
        assert!(again.sanitized().0.show_labels);
    }

    #[test]
    fn guide_settings_are_checked() {
        let mut s = RadarSettings::default();
        s.guide.beep_volume = 4.0;
        s.guide.screen_size = 20.0;
        let (s, w) = s.sanitized();
        assert_eq!(s.guide.beep_volume, 0.25);
        assert_eq!(s.guide.screen_size, 260.0);
        assert_eq!(w.len(), 2, "{w:?}");
        // Settings saved before the beep existed get it, quiet.
        let old: GuideSettings = serde_json::from_str(r#"{"radarArrow":true}"#).unwrap();
        assert!(old.beep);
        assert_eq!(old.beep_volume, 0.25);
    }

    #[test]
    fn bad_colours_and_transforms_fall_back() {
        let mut s = RadarSettings::default();
        s.colors.cube = "red".into();
        s.colors.ring = "#12345".into();
        s.coordinate_transform.scale = -2.0;
        let (s, w) = s.sanitized();
        assert_eq!(s.colors.cube, RadarColors::default().cube);
        assert_eq!(s.colors.ring, RadarColors::default().ring);
        assert_eq!(s.coordinate_transform, CoordinateTransform::default());
        assert_eq!(w.len(), 3);
    }

    #[test]
    fn hex_colour_check() {
        for ok in ["#fff", "#A1b2C3", "#11223344"] {
            assert!(is_hex_color(ok), "{ok}");
        }
        for bad in ["fff", "#ff", "#ggg", "#1234567", ""] {
            assert!(!is_hex_color(bad), "{bad}");
        }
    }

    #[test]
    fn data_source_sanitation() {
        let s = RadarSettings {
            data_source: DataSourceConfig::Replay {
                path: " ".into(),
                speed: 1.0,
                looped: false,
            },
            ..Default::default()
        };
        assert_eq!(s.sanitized().0.data_source, DataSourceConfig::None);

        let s = RadarSettings {
            data_source: DataSourceConfig::LocalTcp { port: 80 },
            ..Default::default()
        };
        assert_eq!(
            s.sanitized().0.data_source,
            DataSourceConfig::LocalTcp { port: 47_810 }
        );
    }

    #[test]
    fn packet_capture_config_reads_and_blank_strings_become_none() {
        let s: RadarSettings = serde_json::from_str(
            r#"{"dataSource": {"kind": "packet_capture", "device": "", "profilePath": "p.json", "record": true}}"#,
        )
        .unwrap();
        assert_eq!(
            s.sanitized().0.data_source,
            DataSourceConfig::PacketCapture {
                device: None,
                file: None,
                decoder: CaptureDecoder::Aion2,
                profile_path: Some("p.json".into()),
                north: CaptureNorth::NegY,
                include_unconfirmed: false,
                record: true
            }
        );
    }

    #[test]
    fn partial_json_fills_in_defaults() {
        let s: RadarSettings =
            serde_json::from_str(r#"{"range": 300, "dataSource": {"kind": "local_tcp"}}"#).unwrap();
        assert_eq!(s.range, 300.0);
        assert_eq!(s.data_source, DataSourceConfig::LocalTcp { port: 47_810 });
        assert_eq!(s.colors, RadarColors::default());
    }

    #[test]
    fn json_round_trip_uses_ui_field_names() {
        let s = RadarSettings {
            data_source: DataSourceConfig::Replay {
                path: "a.jsonl".into(),
                speed: 2.0,
                looped: true,
            },
            ..Default::default()
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["staleTimeoutSecs"], 10.0);
        assert_eq!(json["rotationMode"], "north_up");
        assert_eq!(json["dataSource"]["kind"], "replay");
        assert_eq!(json["dataSource"]["loop"], true);
        let back: RadarSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn tracker_config_follows_the_stale_settings() {
        let s = RadarSettings {
            stale_timeout_secs: 2.5,
            stale_retain_secs: 0.0,
            ..Default::default()
        };
        let c = s.tracker_config();
        assert_eq!(c.stale_timeout_ms, 2_500);
        assert_eq!(c.stale_retain_ms, 0);
    }
}
