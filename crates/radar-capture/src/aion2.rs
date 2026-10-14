//! The AION 2 hidden-cube decoder.
//!
//! **Where the facts come from.** The packet layouts below were established
//! by the `aion2-cube-radar` project (dennisreichenberg, GPL-3.0,
//! `docs/FINDINGS.md`, 2026-10-13) from its own captures of the global client
//! (Steam build 25800038, Verteron): cube positions matched known cube spots
//! to the unit. Their code is not copied; this is an independent
//! implementation of the documented layouts. See `docs/05-aion2-protocol.md`.
//!
//! ```text
//! 34 36 object spawn   id varint, kind u16, template u32, x y z f32, f32, heading f32, 3 B, created u64 ms, …
//! 35 36 object removed id varint, reason u8   (04 opened/destroyed, 01 left the send range)
//! 38 36 open start     object varint, actor varint, 01
//! 3a 36 open done      object varint, actor varint, count u32, item u32   (cube key item 0x259AC523)
//! 21 36 map load       count u32, map u32, …, x y z f32 at 16            (own position)
//! 23 36 teleport       …, x y z f32 at 6                                   (own position)
//! 2a 38 buff record    …, caster varint, 01 00, x y z f32                  (caster position)
//! 2b 38 buff record    …, caster varint, u8, u32, 02, x y z f32            (caster position)
//! 02 38 cast kind 2    actor varint, flag varint, skill u32, seq u8, 02, target varint, heading f32, x y z
//! 0e 38 targeted       entity varint, 02, target varint, u32, u8, u32, x y z (target position)
//! ```
//!
//! A hidden cube is an object spawn whose template is a known cube template.
//! Units are centimetres; map north is world −Y (unconfirmed, configurable).
//!
//! How the radar's rules are met:
//! * **Spawned**: a `34 36` with a cube template.
//! * **Still there**: the server reports objects leaving the send range
//!   (`35 36` reason 01) and being opened (reason 04). While the game stream
//!   keeps arriving with no removal, the cube is still in the server's view,
//!   so a presence confirmation (`cube_seen`) is sent every couple of
//!   seconds. If the stream stalls or bytes are lost, confirmations stop and
//!   every cube is marked lost: nothing is shown as live on a guess.
//! * **Gone**: reason 04 → collected; reason 01 → lost (stale, not live).
//! * **Player position**: only arrives with events (own buffs, being
//!   attacked, map loads). The radar shows how old it is.

use std::collections::{BTreeMap, HashMap, VecDeque};

use radar_core::Millis;
use radar_ingest::{WireEvent, WireId, WireProvenance};
use serde::{Deserialize, Serialize};

use crate::discovery::read_varint;
use crate::framing::Frame;

pub const DEFAULT_CONFIG_JSON: &str = include_str!("aion2-default.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateStatus {
    Verified,
    Hypothesis,
    /// Learned this session: an object of this template was opened with the cube key.
    Learned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeTemplate {
    pub template: u32,
    pub zone: String,
    pub status: TemplateStatus,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Opcodes {
    pub heartbeat: String,
    pub self_info: String,
    pub map_load: String,
    pub teleport: String,
    pub object_spawn: String,
    pub object_removed: String,
    pub open_start: String,
    pub open_done: String,
    pub pos_buff_a: String,
    pub pos_buff_b: String,
    pub pos_cast: String,
    pub pos_target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum North {
    /// Map north = world −Y, east = +X (Cube Watch / aion2-cube-radar default; unconfirmed).
    #[default]
    NegY,
    /// Map north = world +Y, east = +X.
    PosY,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aion2Config {
    #[serde(default)]
    pub client_build: String,
    #[serde(default)]
    pub source: String,
    pub opcodes: Opcodes,
    #[serde(default)]
    pub self_id_opcodes: Vec<String>,
    pub cube_templates: Vec<CubeTemplate>,
    #[serde(default)]
    pub use_hypotheses: bool,
    #[serde(default)]
    pub key_item: Option<u32>,
    #[serde(default)]
    pub map_zones: BTreeMap<String, String>,
    pub units_per_meter: f64,
    #[serde(default)]
    pub north: North,
    pub seen_interval_ms: Millis,
    pub stream_timeout_ms: Millis,
}

impl Aion2Config {
    pub fn builtin() -> Aion2Config {
        serde_json::from_str(DEFAULT_CONFIG_JSON).expect("built-in AION 2 config is valid")
    }

    pub fn from_json(text: &str) -> Result<Aion2Config, String> {
        let c: Aion2Config = serde_json::from_str(text).map_err(|e| e.to_string())?;
        c.compile().map(|_| c)
    }

    fn compile(&self) -> Result<Compiled, String> {
        let op = |name: &str, hex: &str| -> Result<[u8; 2], String> {
            let b = crate::profile::parse_hex(hex).filter(|b| b.len() == 2);
            b.map(|b| [b[0], b[1]])
                .ok_or_else(|| format!("opcode {name}: {hex:?} is not two hex bytes"))
        };
        let o = &self.opcodes;
        if !(self.units_per_meter.is_finite() && self.units_per_meter > 0.0) {
            return Err("unitsPerMeter must be a positive number".into());
        }
        Ok(Compiled {
            heartbeat: op("heartbeat", &o.heartbeat)?,
            self_info: op("selfInfo", &o.self_info)?,
            map_load: op("mapLoad", &o.map_load)?,
            teleport: op("teleport", &o.teleport)?,
            object_spawn: op("objectSpawn", &o.object_spawn)?,
            object_removed: op("objectRemoved", &o.object_removed)?,
            open_start: op("openStart", &o.open_start)?,
            open_done: op("openDone", &o.open_done)?,
            pos_buff_a: op("posBuffA", &o.pos_buff_a)?,
            pos_buff_b: op("posBuffB", &o.pos_buff_b)?,
            pos_cast: op("posCast", &o.pos_cast)?,
            pos_target: op("posTarget", &o.pos_target)?,
            self_id_ops: self
                .self_id_opcodes
                .iter()
                .map(|h| op("selfIdOpcodes", h))
                .collect::<Result<_, _>>()?,
        })
    }
}

struct Compiled {
    heartbeat: [u8; 2],
    self_info: [u8; 2],
    map_load: [u8; 2],
    teleport: [u8; 2],
    object_spawn: [u8; 2],
    object_removed: [u8; 2],
    open_start: [u8; 2],
    open_done: [u8; 2],
    pos_buff_a: [u8; 2],
    pos_buff_b: [u8; 2],
    pos_cast: [u8; 2],
    pos_target: [u8; 2],
    self_id_ops: Vec<[u8; 2]>,
}

// ---------------------------------------------------------------------------
// Message parsing (bodies start after the 2-byte opcode)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectSpawn {
    pub id: u64,
    pub kind: u16,
    pub template: u32,
    pub pos: [f32; 3],
    pub heading: f32,
    /// Unix ms the object was created, when present and plausible.
    pub created_ms: Option<i64>,
}

fn varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let (v, len) = read_varint(b, *at)?;
    if len > 5 {
        return None;
    }
    *at += len;
    Some(v)
}

fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn xyz_at(b: &[u8], at: usize) -> Option<[f32; 3]> {
    Some([f32_at(b, at)?, f32_at(b, at + 4)?, f32_at(b, at + 8)?])
}

/// Finite, inside the map bounds, and not a small vector near the origin
/// (some records carry offsets instead of positions).
pub fn is_world_position(p: [f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite())
        && p[0].abs() < 420_000.0
        && p[1].abs() < 420_000.0
        && p[2].abs() < 120_000.0
        && p[0].hypot(p[1]) > 2_000.0
}

pub fn parse_spawn(b: &[u8]) -> Option<ObjectSpawn> {
    let mut o = 0;
    let id = varint(b, &mut o)?;
    if o + 26 > b.len() {
        return None;
    }
    let kind = u16::from_le_bytes([b[o], b[o + 1]]);
    let template = u32_at(b, o + 2)?;
    let pos = xyz_at(b, o + 6)?;
    if !pos.iter().all(|v| v.is_finite()) {
        return None;
    }
    let heading = f32_at(b, o + 22)?;
    let created_ms = b
        .get(o + 29..o + 37)
        .map(|s| i64::from_le_bytes(s.try_into().expect("8 bytes")))
        .filter(|t| (1_600_000_000_000..2_600_000_000_000).contains(t));
    Some(ObjectSpawn {
        id,
        kind,
        template,
        pos,
        heading,
        created_ms,
    })
}

/// (object id, reason)
pub fn parse_removed(b: &[u8]) -> Option<(u64, u8)> {
    let mut o = 0;
    let id = varint(b, &mut o)?;
    Some((id, *b.get(o)?))
}

/// (object, actor, count, item); count/item only on "done".
pub fn parse_interaction(b: &[u8], done: bool) -> Option<(u64, u64, u32, u32)> {
    let mut o = 0;
    let obj = varint(b, &mut o)?;
    let actor = varint(b, &mut o)?;
    let (count, item) = if done {
        (u32_at(b, o).unwrap_or(0), u32_at(b, o + 4).unwrap_or(0))
    } else {
        (0, 0)
    };
    Some((obj, actor, count, item))
}

/// The caster varint ends `gap` bytes before the trailing x y z. A varint
/// start is only accepted where the previous byte has no continuation bit.
fn caster_tail(b: &[u8], gap: usize) -> Option<(u64, [f32; 3])> {
    let end = b.len().checked_sub(12 + gap)?;
    for w in 1..=3 {
        let Some(start) = end.checked_sub(w) else {
            continue;
        };
        if start < 1 || b[start - 1] & 0x80 != 0 {
            continue;
        }
        let mut o = start;
        if let Some(caster) = varint(b, &mut o) {
            if o == end {
                let p = xyz_at(b, b.len() - 12)?;
                return is_world_position(p).then_some((caster, p));
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixKind {
    BuffA,
    BuffB,
    Cast,
    Target,
}

/// (entity the position belongs to, position)
pub fn parse_position(kind: FixKind, b: &[u8]) -> Option<(u64, [f32; 3])> {
    match kind {
        FixKind::BuffA => {
            if b.len() < 16 || b[b.len() - 14] != 0x01 || b[b.len() - 13] != 0x00 {
                return None;
            }
            caster_tail(b, 2)
        }
        FixKind::BuffB => {
            if b.len() < 20 || b[b.len() - 13] != 0x02 {
                return None;
            }
            caster_tail(b, 6)
        }
        FixKind::Cast => {
            let mut o = 0;
            varint(b, &mut o)?;
            varint(b, &mut o)?;
            o += 5; // skill u32, seq u8
            if *b.get(o)? != 2 {
                return None;
            }
            o += 1;
            let target = varint(b, &mut o)?;
            let p = xyz_at(b, o + 4)?; // after the heading
            is_world_position(p).then_some((target, p))
        }
        FixKind::Target => {
            let mut o = 0;
            varint(b, &mut o)?;
            if *b.get(o)? != 2 {
                return None;
            }
            o += 1;
            let target = varint(b, &mut o)?;
            let p = xyz_at(b, o + 9)?;
            is_world_position(p).then_some((target, p))
        }
    }
}

// ---------------------------------------------------------------------------
// Decoder state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Aion2Status {
    pub client_build: String,
    pub self_id: Option<u64>,
    pub self_fix_age_ms: Option<Millis>,
    pub map_id: Option<u32>,
    pub zone: Option<String>,
    pub cubes_live: usize,
    pub cubes_seen_total: u64,
    pub objects_tracked: usize,
    pub last_object_spawn_ms: Option<Millis>,
    pub learned_templates: Vec<u32>,
    pub warnings: Vec<String>,
}

pub struct Aion2Decoder {
    cfg: Aion2Config,
    ops: Compiled,
    templates: HashMap<u32, CubeTemplate>,
    self_id: Option<u64>,
    self_from_info: bool,
    votes: VecDeque<(u64, u32)>,
    last_fix_by_entity: HashMap<u64, ([f32; 3], Millis)>,
    self_fix_t: Option<Millis>,
    /// Every object of this connection: id → template.
    objects: HashMap<u64, u32>,
    live: BTreeMap<u64, [f32; 3]>,
    map_id: Option<u32>,
    last_heartbeat: Option<Millis>,
    last_seen_emit: Millis,
    last_spawn: Option<Millis>,
    started: Option<Millis>,
    cubes_seen_total: u64,
    learned: Vec<u32>,
}

const MAX_OBJECTS: usize = 20_000;
const MAX_FIXES: usize = 2_000;

impl Aion2Decoder {
    pub fn new(cfg: Aion2Config) -> Result<Aion2Decoder, String> {
        let ops = cfg.compile()?;
        let templates = cfg
            .cube_templates
            .iter()
            .filter(|t| t.status != TemplateStatus::Hypothesis || cfg.use_hypotheses)
            .map(|t| (t.template, t.clone()))
            .collect();
        Ok(Aion2Decoder {
            cfg,
            ops,
            templates,
            self_id: None,
            self_from_info: false,
            votes: VecDeque::new(),
            last_fix_by_entity: HashMap::new(),
            self_fix_t: None,
            objects: HashMap::new(),
            live: BTreeMap::new(),
            map_id: None,
            last_heartbeat: None,
            last_seen_emit: 0,
            last_spawn: None,
            started: None,
            cubes_seen_total: 0,
            learned: Vec::new(),
        })
    }

    pub fn config(&self) -> &Aion2Config {
        &self.cfg
    }

    pub fn is_cube_template(&self, template: u32) -> bool {
        self.templates.contains_key(&template)
    }

    /// Game units → radar world metres (x east, y north, z up).
    fn to_world(&self, p: [f32; 3]) -> [f64; 3] {
        let k = 1.0 / self.cfg.units_per_meter;
        let (x, y, z) = (p[0] as f64 * k, p[1] as f64 * k, p[2] as f64 * k);
        match self.cfg.north {
            North::NegY => [x, -y, z],
            North::PosY => [x, y, z],
        }
    }

    fn zone_name(&self) -> Option<String> {
        let id = self.map_id?;
        Some(match self.cfg.map_zones.get(&id.to_string()) {
            Some(name) => format!("{name} ({id})"),
            None => format!("map {id}"),
        })
    }

    /// A new game connection: entity ids start over.
    pub fn on_new_connection(&mut self) {
        self.self_id = None;
        self.self_from_info = false;
        self.votes.clear();
        self.last_fix_by_entity.clear();
        self.self_fix_t = None;
        self.objects.clear();
        self.live.clear();
    }

    /// Bytes were lost: a removal may have been missed, so no cube can be
    /// vouched for any more.
    pub fn on_gap(&mut self, t: Millis) -> Vec<WireEvent> {
        let ids: Vec<u64> = self.live.keys().copied().collect();
        self.live.clear();
        ids.into_iter()
            .map(|id| WireEvent::CubeLost {
                t,
                id: Some(WireId::Num(id)),
                zone: None,
            })
            .collect()
    }

    /// Presence confirmations while the stream is alive; nothing otherwise.
    pub fn tick(&mut self, t: Millis) -> Vec<WireEvent> {
        let alive = self
            .last_heartbeat
            .is_some_and(|hb| t - hb <= self.cfg.stream_timeout_ms);
        if !alive || self.live.is_empty() || t - self.last_seen_emit < self.cfg.seen_interval_ms {
            return Vec::new();
        }
        self.last_seen_emit = t;
        self.live
            .keys()
            .map(|id| WireEvent::CubeSeen {
                t,
                id: Some(WireId::Num(*id)),
                pos: None,
                zone: None,
                provenance: WireProvenance::Live,
            })
            .collect()
    }

    pub fn on_frame(&mut self, frame: &Frame, t: Millis) -> Vec<WireEvent> {
        self.started.get_or_insert(t);
        let op = frame.opcode;
        let body = &frame.payload[2..];
        let mut out = Vec::new();

        if self.ops.self_id_ops.contains(&op) {
            if let Some(id) = varint(body, &mut 0) {
                self.vote(id, 2, &mut out, t);
            }
        }

        if op == self.ops.heartbeat {
            self.last_heartbeat = Some(t);
        } else if op == self.ops.self_info {
            if let Some(id) = varint(body, &mut 0) {
                self.self_from_info = true;
                self.set_self(id, &mut out, t);
            }
        } else if op == self.ops.map_load {
            if let Some(map) = u32_at(body, 4) {
                if self.map_id != Some(map) {
                    let changed = self.map_id.is_some();
                    self.map_id = Some(map);
                    if changed {
                        self.live.clear();
                        out.push(WireEvent::ZoneChanged {
                            t,
                            zone: self.zone_name().unwrap_or_default(),
                        });
                    }
                }
            }
            if let Some(p) = xyz_at(body, 16).filter(|p| is_world_position(*p)) {
                self.own_fix(p, t, &mut out);
            }
        } else if op == self.ops.teleport {
            if let Some(p) = xyz_at(body, 6).filter(|p| is_world_position(*p)) {
                self.own_fix(p, t, &mut out);
            }
        } else if op == self.ops.object_spawn {
            if let Some(s) = parse_spawn(body) {
                self.last_spawn = Some(t);
                if self.objects.len() >= MAX_OBJECTS {
                    self.objects.clear();
                }
                self.objects.insert(s.id, s.template);
                if self.is_cube_template(s.template) {
                    let tpl = &self.templates[&s.template];
                    let label = match tpl.status {
                        TemplateStatus::Verified => format!("Hidden Cube ({})", tpl.zone),
                        TemplateStatus::Learned => format!("Hidden Cube ({}, learned)", tpl.zone),
                        TemplateStatus::Hypothesis => {
                            format!(
                                "Hidden Cube? ({}, unconfirmed type)",
                                tpl.zone.trim_end_matches('?')
                            )
                        }
                    };
                    self.live.insert(s.id, [s.pos[0], s.pos[1], s.pos[2]]);
                    self.cubes_seen_total += 1;
                    let w = self.to_world(s.pos);
                    out.push(WireEvent::CubeSpawned {
                        t,
                        id: Some(WireId::Num(s.id)),
                        pos: w,
                        label: Some(label),
                        zone: None,
                        provenance: WireProvenance::Live,
                    });
                }
            }
        } else if op == self.ops.object_removed {
            if let Some((id, reason)) = parse_removed(body) {
                self.objects.remove(&id);
                if self.live.remove(&id).is_some() {
                    let id = Some(WireId::Num(id));
                    out.push(match reason {
                        0x04 => WireEvent::CubeCollected { t, id, zone: None },
                        0x01 => WireEvent::CubeLost { t, id, zone: None },
                        _ => WireEvent::CubeDespawned { t, id, zone: None },
                    });
                }
            }
        } else if op == self.ops.open_done {
            if let Some((obj, _actor, count, item)) = parse_interaction(body, true) {
                self.learn(obj, count, item);
            }
        } else if op == self.ops.open_start {
            // Nothing to show: the removal that follows decides.
        } else {
            let kind = if op == self.ops.pos_buff_a {
                Some(FixKind::BuffA)
            } else if op == self.ops.pos_buff_b {
                Some(FixKind::BuffB)
            } else if op == self.ops.pos_cast {
                Some(FixKind::Cast)
            } else if op == self.ops.pos_target {
                Some(FixKind::Target)
            } else {
                None
            };
            if let Some(kind) = kind {
                if let Some((entity, p)) = parse_position(kind, body) {
                    // A buff someone cast on themselves votes for who we are:
                    // our own buffs always reach us.
                    if matches!(kind, FixKind::BuffA | FixKind::BuffB)
                        && varint(body, &mut 0) == Some(entity)
                    {
                        self.vote(entity, 1, &mut out, t);
                    }
                    self.entity_fix(entity, p, t, &mut out);
                }
            }
        }
        out
    }

    fn learn(&mut self, obj: u64, count: u32, item: u32) {
        let Some(key) = self.cfg.key_item else { return };
        if count != 1 || item != key {
            return;
        }
        let Some(&template) = self.objects.get(&obj) else {
            return;
        };
        if self.templates.contains_key(&template) {
            return;
        }
        log::info!(
            "learned cube template {template}: an object of it was opened with the cube key"
        );
        self.learned.push(template);
        self.templates.insert(
            template,
            CubeTemplate {
                template,
                zone: self.zone_name().unwrap_or_else(|| "unknown zone".into()),
                status: TemplateStatus::Learned,
                evidence: "opened with the cube key in this session".into(),
            },
        );
    }

    fn vote(&mut self, id: u64, weight: u32, out: &mut Vec<WireEvent>, t: Millis) {
        self.votes.push_back((id, weight));
        while self.votes.len() > 40 {
            self.votes.pop_front();
        }
        let mut tally: HashMap<u64, u32> = HashMap::new();
        for (v, w) in &self.votes {
            *tally.entry(*v).or_default() += w;
        }
        let total: u32 = tally.values().sum();
        let (best, w) = tally
            .into_iter()
            .max_by_key(|(_, w)| *w)
            .expect("just pushed");
        if Some(best) == self.self_id {
            return;
        }
        let (need_w, need_share) = if self.self_from_info {
            (10, 0.8)
        } else {
            (4, 0.6)
        };
        if w >= need_w && w as f64 >= total as f64 * need_share {
            self.set_self(best, out, t);
        }
    }

    fn set_self(&mut self, id: u64, out: &mut Vec<WireEvent>, now: Millis) {
        if self.self_id == Some(id) {
            return;
        }
        self.self_id = Some(id);
        if let Some(&(p, t)) = self.last_fix_by_entity.get(&id) {
            if now - t < 60_000 {
                self.emit_player(p, t, out);
            }
        }
    }

    fn entity_fix(&mut self, entity: u64, p: [f32; 3], t: Millis, out: &mut Vec<WireEvent>) {
        if self.last_fix_by_entity.len() >= MAX_FIXES {
            self.last_fix_by_entity
                .retain(|_, (_, ft)| t - *ft < 300_000);
            if self.last_fix_by_entity.len() >= MAX_FIXES {
                self.last_fix_by_entity.clear();
            }
        }
        self.last_fix_by_entity.insert(entity, (p, t));
        if self.self_id == Some(entity) {
            self.emit_player(p, t, out);
        }
    }

    /// Map load / teleport: always our own position.
    fn own_fix(&mut self, p: [f32; 3], t: Millis, out: &mut Vec<WireEvent>) {
        if let Some(id) = self.self_id {
            self.last_fix_by_entity.insert(id, (p, t));
        }
        self.emit_player(p, t, out);
    }

    fn emit_player(&mut self, p: [f32; 3], t: Millis, out: &mut Vec<WireEvent>) {
        self.self_fix_t = Some(t);
        out.push(WireEvent::Player {
            t,
            pos: self.to_world(p),
            heading: None, // the game does not send the own facing
            zone: None,
        });
    }

    pub fn status(&self, now: Millis) -> Aion2Status {
        let mut warnings = Vec::new();
        let since_start = self.started.map(|s| now - s).unwrap_or(0);
        if self.last_heartbeat.is_none_or(|hb| now - hb > 10_000) && since_start > 10_000 {
            warnings.push("No heartbeat from the game for 10 s: is the game running and the right adapter captured?".into());
        }
        if self.self_id.is_none() && since_start > 30_000 {
            warnings.push("Own character not identified yet: cast a buff on yourself.".into());
        }
        if since_start > 300_000 && self.last_spawn.is_none_or(|s| now - s > 300_000) {
            warnings.push(
                "No object spawns for 5 minutes. If you were moving, a game patch may have changed the opcodes (see aion2-decoder.json)."
                    .into(),
            );
        }
        Aion2Status {
            client_build: self.cfg.client_build.clone(),
            self_id: self.self_id,
            self_fix_age_ms: self.self_fix_t.map(|t| now - t),
            map_id: self.map_id,
            zone: self.zone_name(),
            cubes_live: self.live.len(),
            cubes_seen_total: self.cubes_seen_total,
            objects_tracked: self.objects.len(),
            last_object_spawn_ms: self.last_spawn,
            learned_templates: self.learned.clone(),
            warnings,
        }
    }
}

#[cfg(test)]
mod tests;
