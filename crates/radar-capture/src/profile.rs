//! Decoder profiles: which packets mean what, as data instead of code.
//!
//! The cube packet layout is not known in advance, so it is not compiled in.
//! Once the discovery tools have found it, it is written down in a JSON
//! profile and the radar decodes with that. The profile carries a
//! `validated` flag: the app shows nothing from a profile until a person has
//! checked its output against the game and set that flag.
//!
//! ```json
//! {
//!   "name": "AION 2 global — cube spawns",
//!   "gameVersion": "client build you tested",
//!   "validated": false,
//!   "messages": [
//!     { "opcode": "12 34", "event": "cube_spawned",
//!       "when": [ { "offset": 9, "equals": "AB CD" } ],
//!       "fields": {
//!         "id": { "offset": 2, "type": "varint" },
//!         "x":  { "after": "id", "offset": 4, "type": "f32le" },
//!         "y":  { "after": "id", "offset": 8, "type": "f32le" },
//!         "z":  { "after": "id", "offset": 12, "type": "f32le" } } }
//!   ]
//! }
//! ```
//!
//! Offsets count from the first opcode byte. `after` makes an offset
//! relative to the end of an earlier field (fields are read in the order id,
//! x, y, z, heading), which handles a variable-length varint before them.

use std::collections::HashMap;
use std::path::Path;

use radar_core::Millis;
use radar_ingest::{WireEvent, WireId, WireProvenance};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::discovery::read_varint;
use crate::framing::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleEvent {
    Player,
    CubeSpawned,
    CubeSeen,
    CubeDespawned,
    CubeCollected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    U8,
    U16le,
    U32le,
    U64le,
    I16le,
    I32le,
    F32le,
    F64le,
    Varint,
}

impl FieldType {
    fn read(self, p: &[u8], at: usize) -> Option<(f64, usize)> {
        fn arr<const N: usize>(p: &[u8], at: usize) -> Option<[u8; N]> {
            p.get(at..at + N)?.try_into().ok()
        }
        Some(match self {
            Self::U8 => (*p.get(at)? as f64, 1),
            Self::U16le => (u16::from_le_bytes(arr(p, at)?) as f64, 2),
            Self::U32le => (u32::from_le_bytes(arr(p, at)?) as f64, 4),
            Self::U64le => (u64::from_le_bytes(arr(p, at)?) as f64, 8),
            Self::I16le => (i16::from_le_bytes(arr(p, at)?) as f64, 2),
            Self::I32le => (i32::from_le_bytes(arr(p, at)?) as f64, 4),
            Self::F32le => (f32::from_le_bytes(arr(p, at)?) as f64, 4),
            Self::F64le => (f64::from_le_bytes(arr(p, at)?), 8),
            Self::Varint => {
                let (v, l) = read_varint(p, at)?;
                (v as f64, l)
            }
        })
    }

    /// Integer types only (checked by the profile), read without rounding.
    fn read_int(self, p: &[u8], at: usize) -> Option<(u64, usize)> {
        fn arr<const N: usize>(p: &[u8], at: usize) -> Option<[u8; N]> {
            p.get(at..at + N)?.try_into().ok()
        }
        Some(match self {
            Self::U8 => (*p.get(at)? as u64, 1),
            Self::U16le => (u16::from_le_bytes(arr(p, at)?) as u64, 2),
            Self::U32le => (u32::from_le_bytes(arr(p, at)?) as u64, 4),
            Self::U64le => (u64::from_le_bytes(arr(p, at)?), 8),
            Self::I16le => (i16::from_le_bytes(arr(p, at)?) as i64 as u64, 2),
            Self::I32le => (i32::from_le_bytes(arr(p, at)?) as i64 as u64, 4),
            Self::Varint => read_varint(p, at)?,
            Self::F32le | Self::F64le => return None,
        })
    }

    fn is_integer(self) -> bool {
        !matches!(self, Self::F32le | Self::F64le)
    }
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    pub offset: usize,
    #[serde(rename = "type")]
    pub ty: FieldType,
    /// Name of an earlier field; `offset` then counts from its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Multiplier applied to the raw value (e.g. a u16 angle to degrees).
    #[serde(default = "one")]
    pub scale: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Fields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<FieldSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<FieldSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<FieldSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<FieldSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<FieldSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ByteMatch {
    pub offset: usize,
    /// Hex bytes, e.g. "AB CD".
    pub equals: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRule {
    pub opcode: String,
    pub event: RuleEvent,
    #[serde(default)]
    pub when: Vec<ByteMatch>,
    pub fields: Fields,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecoderProfile {
    pub name: String,
    #[serde(default)]
    pub game_version: Option<String>,
    /// Set by a person after checking decoded output against the game.
    #[serde(default)]
    pub validated: bool,
    #[serde(default)]
    pub notes: String,
    pub messages: Vec<MessageRule>,
}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("cannot read profile: {0}")]
    Io(#[from] std::io::Error),
    #[error("profile is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("profile is invalid: {0}")]
    Invalid(String),
}

pub fn parse_hex(s: &str) -> Option<Vec<u8>> {
    let digits: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return None;
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).ok())
        .collect()
}

struct CompiledRule {
    rule: MessageRule,
    when: Vec<(usize, Vec<u8>)>,
}

/// A profile ready to decode frames.
pub struct Decoder {
    profile: DecoderProfile,
    by_opcode: HashMap<[u8; 2], Vec<CompiledRule>>,
    /// Frames of a profiled opcode whose fields could not be read.
    pub field_errors: u64,
    pub decoded: u64,
}

impl DecoderProfile {
    pub fn load(path: &Path) -> Result<Self, ProfileError> {
        let p: DecoderProfile = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        p.check()?;
        Ok(p)
    }

    /// Structural checks: opcodes parse, each event has the fields it needs,
    /// `after` refers to an earlier field.
    pub fn check(&self) -> Result<(), ProfileError> {
        let bad =
            |i: usize, m: String| Err(ProfileError::Invalid(format!("message {}: {m}", i + 1)));
        for (i, r) in self.messages.iter().enumerate() {
            if parse_hex(&r.opcode).is_none_or(|b| b.len() != 2) {
                return bad(i, format!("opcode {:?} must be two hex bytes", r.opcode));
            }
            for w in &r.when {
                if parse_hex(&w.equals).is_none() {
                    return bad(i, format!("`when` value {:?} is not hex", w.equals));
                }
            }
            let f = &r.fields;
            let need: &[(&str, bool)] = match r.event {
                RuleEvent::Player => &[("x", f.x.is_some()), ("y", f.y.is_some())],
                RuleEvent::CubeSpawned => &[
                    ("id", f.id.is_some()),
                    ("x", f.x.is_some()),
                    ("y", f.y.is_some()),
                ],
                _ => &[("id", f.id.is_some())],
            };
            for (name, present) in need {
                if !present {
                    return bad(i, format!("{:?} needs field {name}", r.event));
                }
            }
            if let Some(id) = &f.id {
                if !id.ty.is_integer() {
                    return bad(i, "id must be an integer type".into());
                }
            }
            let order = ["id", "x", "y", "z", "heading"];
            for (pos, (name, spec)) in order.iter().zip(field_list(f)).enumerate() {
                let Some(spec) = spec else { continue };
                if !(spec.scale.is_finite() && spec.scale != 0.0) {
                    return bad(i, format!("field {name}: scale must be a non-zero number"));
                }
                if let Some(after) = &spec.after {
                    let earlier = order[..pos].iter().position(|n| n == after);
                    if earlier.is_none_or(|e| field_list(f)[e].is_none()) {
                        return bad(
                            i,
                            format!("field {name}: `after` must name an earlier, defined field"),
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

fn field_list(f: &Fields) -> [Option<&FieldSpec>; 5] {
    [
        f.id.as_ref(),
        f.x.as_ref(),
        f.y.as_ref(),
        f.z.as_ref(),
        f.heading.as_ref(),
    ]
}

impl Decoder {
    pub fn new(profile: DecoderProfile) -> Result<Self, ProfileError> {
        profile.check()?;
        let mut by_opcode: HashMap<[u8; 2], Vec<CompiledRule>> = HashMap::new();
        for rule in &profile.messages {
            let op = parse_hex(&rule.opcode).expect("checked");
            let when = rule
                .when
                .iter()
                .map(|w| (w.offset, parse_hex(&w.equals).expect("checked")))
                .collect();
            by_opcode
                .entry([op[0], op[1]])
                .or_default()
                .push(CompiledRule {
                    rule: rule.clone(),
                    when,
                });
        }
        Ok(Self {
            profile,
            by_opcode,
            field_errors: 0,
            decoded: 0,
        })
    }

    pub fn profile(&self) -> &DecoderProfile {
        &self.profile
    }

    pub fn opcodes(&self) -> impl Iterator<Item = [u8; 2]> + '_ {
        self.by_opcode.keys().copied()
    }

    /// Decodes a frame with the first rule whose `when` conditions match.
    pub fn decode(&mut self, frame: &Frame, t: Millis) -> Option<WireEvent> {
        let rules = self.by_opcode.get(&frame.opcode)?;
        let p = &frame.payload;
        let rule = rules.iter().find(|r| {
            r.when
                .iter()
                .all(|(off, bytes)| p.get(*off..*off + bytes.len()) == Some(bytes))
        })?;
        match read_fields(&rule.rule.fields, p) {
            Some(v) => {
                let ev = build(&rule.rule, v, t);
                if ev.is_some() {
                    self.decoded += 1;
                } else {
                    self.field_errors += 1;
                }
                ev
            }
            None => {
                self.field_errors += 1;
                None
            }
        }
    }
}

/// The id (exact, as an integer) and x, y, z, heading.
type Values = (Option<u64>, [Option<f64>; 4]);

fn read_fields(f: &Fields, p: &[u8]) -> Option<Values> {
    let names = ["id", "x", "y", "z", "heading"];
    let mut ends: [Option<usize>; 5] = [None; 5];
    let mut id = None;
    let mut vals: [Option<f64>; 4] = [None; 4];
    for (i, spec) in field_list(f).into_iter().enumerate() {
        let Some(spec) = spec else { continue };
        let base = match &spec.after {
            Some(a) => ends[names.iter().position(|n| n == a)?]?,
            None => 0,
        };
        let at = base + spec.offset;
        if i == 0 {
            // Ids are read exactly: an f64 would round ids above 2^53.
            let (raw, len) = spec.ty.read_int(p, at)?;
            id = Some(raw);
            ends[0] = Some(at + len);
        } else {
            let (raw, len) = spec.ty.read(p, at)?;
            vals[i - 1] = Some(raw * spec.scale);
            ends[i] = Some(at + len);
        }
    }
    Some((id, vals))
}

fn build(rule: &MessageRule, (id, v): Values, t: Millis) -> Option<WireEvent> {
    let id = id.map(WireId::Num);
    let finite = |x: Option<f64>| x.filter(|x| x.is_finite());
    let pos = || Some([finite(v[0])?, finite(v[1])?, finite(v[2]).unwrap_or(0.0)]);
    Some(match rule.event {
        RuleEvent::Player => WireEvent::Player {
            accuracy: None,
            height_accuracy: None,
            t,
            pos: pos()?,
            heading: finite(v[3]),
            zone: None,
        },
        RuleEvent::CubeSpawned => WireEvent::CubeSpawned {
            t,
            id,
            pos: pos()?,
            label: Some(rule.label.clone().unwrap_or_else(|| "Cube".into())),
            zone: None,
            provenance: WireProvenance::Live,
        },
        RuleEvent::CubeSeen => WireEvent::CubeSeen {
            t,
            id,
            pos: pos(),
            zone: None,
            provenance: WireProvenance::Live,
        },
        RuleEvent::CubeDespawned => WireEvent::CubeDespawned { t, id, zone: None },
        RuleEvent::CubeCollected => WireEvent::CubeCollected { t, id, zone: None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(json: &str) -> Result<DecoderProfile, ProfileError> {
        let p: DecoderProfile = serde_json::from_str(json)?;
        p.check()?;
        Ok(p)
    }

    const TEST_PROFILE: &str = r#"{
      "name": "test — fictional opcodes",
      "validated": true,
      "messages": [
        { "opcode": "C1 7E", "event": "cube_spawned", "when": [{"offset": 2, "equals": "01"}],
          "fields": { "id": {"offset": 3, "type": "varint"},
                      "x": {"after": "id", "offset": 0, "type": "f32le"},
                      "y": {"after": "id", "offset": 4, "type": "f32le"},
                      "z": {"after": "id", "offset": 8, "type": "f32le"} } },
        { "opcode": "C1 7E", "event": "cube_collected", "when": [{"offset": 2, "equals": "02"}],
          "fields": { "id": {"offset": 3, "type": "varint"} } },
        { "opcode": "C2 7E", "event": "player",
          "fields": { "x": {"offset": 2, "type": "f32le"}, "y": {"offset": 6, "type": "f32le"},
                      "heading": {"offset": 10, "type": "u16le", "scale": 0.0054931640625} } }
      ]
    }"#;

    fn frame(payload: Vec<u8>) -> Frame {
        Frame {
            opcode: [payload[0], payload[1]],
            payload,
            bundled: false,
        }
    }

    #[test]
    fn decodes_with_varint_relative_offsets_and_conditions() {
        let mut d = Decoder::new(profile(TEST_PROFILE).unwrap()).unwrap();
        let mut spawn = vec![0xC1, 0x7E, 0x01, 0xAC, 0x02]; // id varint 300
        for v in [10.5f32, -20.0, 3.0] {
            spawn.extend(v.to_le_bytes());
        }
        let Some(WireEvent::CubeSpawned { id, pos, .. }) = d.decode(&frame(spawn), 5) else {
            panic!("spawn not decoded")
        };
        assert_eq!(id, Some(WireId::Num(300)));
        assert_eq!(pos, [10.5, -20.0, 3.0]);

        let collected = d.decode(&frame(vec![0xC1, 0x7E, 0x02, 0x05]), 6);
        assert!(matches!(
            collected,
            Some(WireEvent::CubeCollected {
                id: Some(WireId::Num(5)),
                ..
            })
        ));

        // Neither condition matches: not decoded at all.
        assert!(d.decode(&frame(vec![0xC1, 0x7E, 0x09, 0x05]), 7).is_none());

        let mut player = vec![0xC2, 0x7E];
        player.extend(1.0f32.to_le_bytes());
        player.extend(2.0f32.to_le_bytes());
        player.extend(16384u16.to_le_bytes()); // 90 degrees
        let Some(WireEvent::Player { heading, .. }) = d.decode(&frame(player), 8) else {
            panic!()
        };
        assert!((heading.unwrap() - 90.0).abs() < 1e-9);
        assert_eq!(d.decoded, 3);
    }

    #[test]
    fn short_or_nan_packets_are_errors_not_events() {
        let mut d = Decoder::new(profile(TEST_PROFILE).unwrap()).unwrap();
        assert!(d
            .decode(&frame(vec![0xC1, 0x7E, 0x01, 0x05, 0, 0]), 1)
            .is_none());
        let mut nan = vec![0xC1, 0x7E, 0x01, 0x05];
        for v in [f32::NAN, 1.0, 1.0] {
            nan.extend(v.to_le_bytes());
        }
        assert!(d.decode(&frame(nan), 1).is_none());
        assert_eq!(d.field_errors, 2);
    }

    #[test]
    fn structural_mistakes_are_reported() {
        let missing_pos = r#"{"name":"x","messages":[{"opcode":"01 02","event":"cube_spawned","fields":{"id":{"offset":2,"type":"u32le"}}}]}"#;
        assert!(matches!(
            profile(missing_pos),
            Err(ProfileError::Invalid(_))
        ));
        let bad_op = r#"{"name":"x","messages":[{"opcode":"0102 03","event":"cube_seen","fields":{"id":{"offset":2,"type":"u32le"}}}]}"#;
        assert!(matches!(profile(bad_op), Err(ProfileError::Invalid(_))));
        let float_id = r#"{"name":"x","messages":[{"opcode":"01 02","event":"cube_seen","fields":{"id":{"offset":2,"type":"f32le"}}}]}"#;
        assert!(matches!(profile(float_id), Err(ProfileError::Invalid(_))));
        let bad_after = r#"{"name":"x","messages":[{"opcode":"01 02","event":"player","fields":{"x":{"offset":2,"type":"f32le","after":"y"},"y":{"offset":6,"type":"f32le"}}}]}"#;
        assert!(matches!(profile(bad_after), Err(ProfileError::Invalid(_))));
        assert!(profile(TEST_PROFILE).is_ok());
    }
}
