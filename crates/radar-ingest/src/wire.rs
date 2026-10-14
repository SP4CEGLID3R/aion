//! Radar Event Protocol v1: the documented, structured format every data
//! source is converted into before the adapter. See
//! `docs/radar-event-protocol.md`.
//!
//! One JSON object per line:
//!
//! ```text
//! {"v":1,"type":"cube_spawned","t":1760000000000,"id":"c-17","pos":[12.5,40,3]}
//! ```
//!
//! Blank lines and lines starting with `#` are ignored.

use radar_core::Millis;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROTOCOL_VERSION: u64 = 1;
/// Lines longer than this are rejected without being parsed.
pub const MAX_LINE_BYTES: usize = 16 * 1024;

/// A source-side identifier: string or non-negative integer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WireId {
    Str(String),
    Num(u64),
}

impl WireId {
    pub fn into_string(self) -> String {
        match self {
            Self::Str(s) => s,
            Self::Num(n) => n.to_string(),
        }
    }
}

impl From<&str> for WireId {
    fn from(s: &str) -> Self {
        Self::Str(s.to_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WireProvenance {
    #[default]
    Live,
    Historical,
    Predicted,
}

/// One structured event, as produced by a parser or telemetry tool.
/// Positions are in the *source's* coordinate frame; the adapter maps them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireEvent {
    Player {
        t: Millis,
        pos: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heading: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
    },
    CubeSpawned {
        t: Millis,
        #[serde(default)]
        id: Option<WireId>,
        pos: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
        #[serde(default)]
        provenance: WireProvenance,
    },
    CubeSeen {
        t: Millis,
        #[serde(default)]
        id: Option<WireId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pos: Option<[f64; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
        #[serde(default)]
        provenance: WireProvenance,
    },
    CubeDespawned {
        t: Millis,
        #[serde(default)]
        id: Option<WireId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
    },
    CubeCollected {
        t: Millis,
        #[serde(default)]
        id: Option<WireId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
    },
    /// The source can no longer confirm the cube (e.g. out of the server's
    /// send range). The radar marks it stale, never live.
    CubeLost {
        t: Millis,
        #[serde(default)]
        id: Option<WireId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
    },
    ZoneChanged {
        t: Millis,
        zone: String,
    },
    /// Static reference data: a place cubes *can* spawn. Accepted so it can
    /// be counted and logged; never shown.
    SpawnPoint {
        #[serde(default)]
        t: Option<Millis>,
        #[serde(default)]
        id: Option<WireId>,
        pos: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
    },
    /// A past location (from a log or database). Never shown as live.
    HistoricalLocation {
        t: Millis,
        #[serde(default)]
        id: Option<WireId>,
        pos: [f64; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        zone: Option<String>,
    },
}

impl WireEvent {
    /// The event's source timestamp, if it has one.
    pub fn time(&self) -> Option<Millis> {
        match self {
            Self::Player { t, .. }
            | Self::CubeSpawned { t, .. }
            | Self::CubeSeen { t, .. }
            | Self::CubeDespawned { t, .. }
            | Self::CubeCollected { t, .. }
            | Self::CubeLost { t, .. }
            | Self::ZoneChanged { t, .. }
            | Self::HistoricalLocation { t, .. } => Some(*t),
            Self::SpawnPoint { t, .. } => *t,
        }
    }

    /// Shifts every timestamp by `delta` (used by replay to move a recording
    /// to the present).
    pub fn shift_time(&mut self, delta: Millis) {
        match self {
            Self::Player { t, .. }
            | Self::CubeSpawned { t, .. }
            | Self::CubeSeen { t, .. }
            | Self::CubeDespawned { t, .. }
            | Self::CubeCollected { t, .. }
            | Self::CubeLost { t, .. }
            | Self::ZoneChanged { t, .. }
            | Self::HistoricalLocation { t, .. } => *t += delta,
            Self::SpawnPoint { t, .. } => {
                if let Some(t) = t {
                    *t += delta;
                }
            }
        }
    }

    /// Serialises as one protocol line (without the newline).
    pub fn to_line(&self) -> String {
        let mut value = serde_json::to_value(self).expect("wire events always serialise");
        if let serde_json::Value::Object(map) = &mut value {
            map.insert("v".into(), PROTOCOL_VERSION.into());
        }
        value.to_string()
    }
}

#[derive(Debug, Error)]
pub enum WireError {
    #[error("line is {0} bytes, over the {MAX_LINE_BYTES}-byte limit")]
    TooLong(usize),
    #[error("not valid JSON: {0}")]
    Json(String),
    #[error("missing or unsupported protocol version (expected \"v\": {PROTOCOL_VERSION})")]
    Version,
    #[error("invalid event: {0}")]
    Event(String),
}

/// Parses one protocol line. `Ok(None)` for blank and comment lines.
pub fn parse_line(line: &str) -> Result<Option<WireEvent>, WireError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(WireError::TooLong(line.len()));
    }
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let mut value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| WireError::Json(e.to_string()))?;
    let Some(obj) = value.as_object_mut() else {
        return Err(WireError::Event("expected a JSON object".into()));
    };
    match obj.remove("v").and_then(|v| v.as_u64()) {
        Some(PROTOCOL_VERSION) => {}
        _ => return Err(WireError::Version),
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|e| WireError::Event(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_event_type() {
        let lines = [
            r#"{"v":1,"type":"player","t":5,"pos":[1,2,3],"heading":90,"zone":"z"}"#,
            r#"{"v":1,"type":"cube_spawned","t":5,"id":"c1","pos":[1,2,3],"label":"Cube"}"#,
            r#"{"v":1,"type":"cube_seen","t":6,"id":17}"#,
            r#"{"v":1,"type":"cube_despawned","t":7,"id":"c1"}"#,
            r#"{"v":1,"type":"cube_collected","t":7,"id":"c1"}"#,
            r#"{"v":1,"type":"zone_changed","t":8,"zone":"dungeon"}"#,
            r#"{"v":1,"type":"spawn_point","pos":[1,2,3]}"#,
            r#"{"v":1,"type":"historical_location","t":1,"id":"old","pos":[1,2,3]}"#,
        ];
        for line in lines {
            assert!(parse_line(line).unwrap().is_some(), "{line}");
        }
        let Some(WireEvent::CubeSeen { id, provenance, .. }) = parse_line(lines[2]).unwrap() else {
            panic!()
        };
        assert_eq!(id, Some(WireId::Num(17)));
        assert_eq!(provenance, WireProvenance::Live);
    }

    #[test]
    fn skips_blank_and_comment_lines() {
        assert!(parse_line("").unwrap().is_none());
        assert!(parse_line("   ").unwrap().is_none());
        assert!(parse_line("# a comment").unwrap().is_none());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(parse_line("{nope"), Err(WireError::Json(_))));
        assert!(matches!(parse_line("[1,2]"), Err(WireError::Event(_))));
        assert!(matches!(
            parse_line(r#"{"type":"cube_seen","t":1}"#),
            Err(WireError::Version)
        ));
        assert!(matches!(
            parse_line(r#"{"v":2,"type":"cube_seen","t":1}"#),
            Err(WireError::Version)
        ));
        assert!(matches!(
            parse_line(r#"{"v":1,"type":"teleport","t":1}"#),
            Err(WireError::Event(_))
        ));
        assert!(matches!(
            parse_line(r#"{"v":1,"type":"cube_spawned","t":1,"id":"a"}"#),
            Err(WireError::Event(_))
        ));
        let long = format!(r#"{{"v":1,"pad":"{}"}}"#, "x".repeat(MAX_LINE_BYTES));
        assert!(matches!(parse_line(&long), Err(WireError::TooLong(_))));
    }

    #[test]
    fn round_trips_through_a_line() {
        let ev = WireEvent::CubeSpawned {
            t: 42,
            id: Some("c".into()),
            pos: [1.5, -2.0, 0.0],
            label: None,
            zone: Some("z".into()),
            provenance: WireProvenance::Live,
        };
        let line = ev.to_line();
        assert!(line.contains("\"v\":1"));
        assert_eq!(parse_line(&line).unwrap(), Some(ev));
    }

    #[test]
    fn shift_time_moves_every_timestamp() {
        let mut ev = WireEvent::ZoneChanged {
            t: 10,
            zone: "a".into(),
        };
        ev.shift_time(5);
        assert_eq!(ev.time(), Some(15));
        let mut sp = WireEvent::SpawnPoint {
            t: None,
            id: None,
            pos: [0.0; 3],
            label: None,
            zone: None,
        };
        sp.shift_time(5);
        assert_eq!(sp.time(), None);
    }
}
