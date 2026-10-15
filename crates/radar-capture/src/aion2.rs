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
//! * **Player position**: the server reports it only with events (own
//!   buffs, opening the wings, being attacked, map loads). In between, the
//!   radar follows you with whichever of these works, best first:
//!   1. the in-game minimap read from the screen (`minimap`): last fix plus
//!      how far the minimap has scrolled since, its scale learned from the
//!      fixes;
//!   2. an estimate from objects entering and leaving the send range
//!      (`edge`), checked against every server fix and shown with its
//!      uncertainty.
//!
//!   The client's own movement reports (`movement`) would be best, but the
//!   global client encrypts what it sends, so that learner is off by default.

use std::collections::{BTreeMap, HashMap, VecDeque};

use radar_core::Millis;
use radar_ingest::{WireEvent, WireId, WireProvenance};
use serde::{Deserialize, Serialize};

use crate::discovery::read_varint;
use crate::framing::Frame;

mod edge;
mod minimap;
mod movement;

pub use edge::{Edge, EdgeEstimator, EstimateStatus, RadiusSeed};
pub use minimap::{LocKind, MinimapCalibration, MinimapFusion, MinimapStatus};
pub use movement::{Anchor, MoveSource, MovementLearner, MovementStatus};

use crate::minimap::{MinimapSample, MinimapView};

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
    /// Learn which client packet carries the own movement (see `movement`).
    /// Off by default: the global client encrypts its traffic (the length
    /// prefixes are readable, the contents are not), so there is nothing to
    /// find there.
    #[serde(default)]
    pub learn_movement: bool,
    /// A known client movement field, used right away instead of learning.
    #[serde(default)]
    pub own_movement: Option<MoveSource>,
    /// Estimate the own position between server fixes from objects entering
    /// and leaving the send range (see `edge`).
    #[serde(default = "yes")]
    pub estimate_position: bool,
    /// Send radii per object template (game units), used before they are
    /// learned. Keys are template numbers.
    #[serde(default)]
    pub object_radii: BTreeMap<String, RadiusSeed>,
    /// A movement field learned in an earlier session: tried first, and
    /// dropped like any learned one if the server disagrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movement_hint: Option<MoveSource>,
    /// Minimap calibration from an earlier session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimap_hint: Option<MinimapCalibration>,
    /// Radii measured in earlier sessions (kept apart from the seeds so only
    /// measurements are saved again).
    #[serde(skip)]
    pub learned_radii: BTreeMap<String, RadiusSeed>,
}

/// What the decoder learned in game, kept for the next session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aion2Learned {
    #[serde(default)]
    pub object_radii: BTreeMap<String, RadiusSeed>,
    #[serde(default)]
    pub own_movement: Option<MoveSource>,
    #[serde(default)]
    pub minimap: Option<MinimapCalibration>,
}

impl Aion2Learned {
    pub fn is_empty(&self) -> bool {
        self.object_radii.is_empty() && self.own_movement.is_none() && self.minimap.is_none()
    }
}

fn yes() -> bool {
    true
}

impl Aion2Config {
    pub fn builtin() -> Aion2Config {
        serde_json::from_str(DEFAULT_CONFIG_JSON).expect("built-in AION 2 config is valid")
    }

    /// Uses radii and the movement field learned in earlier sessions.
    /// Measured radii replace the built-in seeds.
    pub fn apply_learned(&mut self, learned: &Aion2Learned) {
        for (k, r) in &learned.object_radii {
            let e = self.object_radii.entry(k.clone()).or_default();
            e.enter = r.enter.or(e.enter);
            e.leave = r.leave.or(e.leave);
        }
        self.learned_radii = learned.object_radii.clone();
        if self.own_movement.is_none() {
            self.movement_hint = learned.own_movement.clone();
        }
        if learned.minimap.is_some() {
            self.minimap_hint = learned.minimap.clone();
        }
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
    /// Where the radar's own position comes from right now: "minimap",
    /// "server" (an event), "estimate", "client" or "none".
    pub position_source: String,
    pub position_age_ms: Option<Millis>,
    pub minimap: MinimapStatus,
    pub movement: MovementStatus,
    pub estimate: EstimateStatus,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PosSource {
    Client,
    Server,
    Minimap,
    Estimate,
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
    /// Every object of this connection: id → (template, position).
    objects: HashMap<u64, (u32, [f32; 3])>,
    live: BTreeMap<u64, [f32; 3]>,
    map_id: Option<u32>,
    last_heartbeat: Option<Millis>,
    last_seen_emit: Millis,
    last_spawn: Option<Millis>,
    started: Option<Millis>,
    cubes_seen_total: u64,
    learned: Vec<u32>,
    movement: MovementLearner,
    edge: EdgeEstimator,
    /// (server unix ms, local t) from the last heartbeat.
    server_clock: Option<(i64, Millis)>,
    last_t: Millis,
    /// Last emitted own position, its time and where it came from.
    last_pos: Option<([f32; 3], Millis, PosSource)>,
    last_client_t: Option<Millis>,
    last_estimate_t: Millis,
    minimap: MinimapFusion,
    last_minimap_t: Option<Millis>,
    /// Where the server last put us: positions between fixes keep its height.
    last_fix_pos: Option<[f32; 3]>,
}

/// Client movement reports newer than this win over estimates.
const CLIENT_FRESH_MS: Millis = 1_500;
/// Minimap positions newer than this win over estimates.
const MINIMAP_FRESH_MS: Millis = 1_500;
/// Estimates are moved forward this often between events.
const ESTIMATE_EVERY_MS: Millis = 250;
/// Own height between server fixes (game units): the minimap and the range
/// estimate only move us across the map, so the height is the last fix's.
/// Its 1-sigma error: the fix's own, plus the ground rising or falling by up
/// to this slope over the distance moved since; unknown without a fix.
const HEIGHT_FIX_ERR: f64 = 50.0;
const HEIGHT_SLOPE: f64 = 0.2;
const HEIGHT_UNKNOWN: f64 = 5_000.0;

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
            movement: MovementLearner::new(
                cfg.learn_movement,
                cfg.own_movement.as_ref(),
                cfg.movement_hint.as_ref(),
            ),
            edge: EdgeEstimator::new(&cfg.object_radii),
            minimap: MinimapFusion::new(cfg.units_per_meter, cfg.minimap_hint.as_ref()),
            last_minimap_t: None,
            last_fix_pos: None,
            cfg,
            server_clock: None,
            last_t: 0,
            last_pos: None,
            last_client_t: None,
            last_estimate_t: 0,
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
        self.movement.on_new_connection();
        self.edge.on_new_connection(self.last_t);
        self.minimap.on_new_connection();
        self.last_pos = None;
        self.last_client_t = None;
        self.last_minimap_t = None;
        self.last_estimate_t = 0;
        self.last_fix_pos = None;
    }

    /// What was learned in game (with earlier sessions' results), to keep
    /// for the next session.
    pub fn learned(&self) -> Aion2Learned {
        // Only measurements are kept (earlier sessions' and this one's);
        // seeds from the configuration stay in the configuration.
        let mut object_radii = self.cfg.learned_radii.clone();
        for (k, r) in self.edge.learned_radii() {
            let e = object_radii.entry(k).or_default();
            e.enter = r.enter.or(e.enter);
            e.leave = r.leave.or(e.leave);
        }
        let minimap = if self.minimap.calibration().legs > 0 {
            Some(self.minimap.calibration())
        } else {
            self.cfg.minimap_hint.clone()
        };
        Aion2Learned {
            object_radii,
            own_movement: self.movement.learned_source(),
            minimap,
        }
    }

    /// The minimap scale for drawing it, and the tracking state.
    pub fn minimap_view(&self) -> MinimapView {
        let st = self.minimap.status(self.last_t);
        MinimapView {
            px_per_m: st.px_per_m,
            calibrated: self.minimap.calibrated(),
            state: st.state,
            reason: st.reason,
            camera_deg: self.camera_heading(self.last_t),
            legs: st.legs,
        }
    }

    fn minimap_fresh(&self, t: Millis) -> bool {
        self.last_minimap_t
            .is_some_and(|m| t - m <= MINIMAP_FRESH_MS)
    }

    /// One minimap measurement from the screen tracker.
    pub fn on_minimap(&mut self, s: &MinimapSample) -> Vec<WireEvent> {
        let mut out = Vec::new();
        self.last_t = self.last_t.max(s.t);
        self.minimap.on_frame(s);
        let loc = self.minimap.locate(s.t);
        let (LocKind::Live, Some(p)) = (loc.kind, loc.pos) else {
            return out;
        };
        if loc.reason == "fresh fix" {
            return out; // the fix itself was already shown
        }
        // Dense truth: lets the range estimator learn object radii.
        self.edge.on_truth(p, s.t, loc.sigma, false);
        if self.last_pos.is_none_or(|(_, lt, _)| s.t >= lt) {
            let heading = self.camera_heading(s.t);
            self.emit_player_as(
                p,
                s.t,
                &mut out,
                PosSource::Minimap,
                Some(loc.sigma),
                heading,
            );
            self.last_minimap_t = Some(s.t);
        }
        out
    }

    /// Camera direction (compass) from the minimap's view cone.
    fn camera_heading(&self, t: Millis) -> Option<f64> {
        let deg = self.minimap.camera(t)?;
        let (dx, dy) = self.minimap.screen_dir_to_world(deg);
        Some(self.heading_of(dx, dy))
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
        let mut out = self.tick_estimate(t);
        out.extend(self.tick_seen(t));
        out
    }

    fn client_fresh(&self, t: Millis) -> bool {
        self.last_client_t.is_some_and(|c| t - c <= CLIENT_FRESH_MS)
    }

    /// Moves a shown estimate forward between events.
    fn tick_estimate(&mut self, t: Millis) -> Vec<WireEvent> {
        let mut out = Vec::new();
        if !self.cfg.estimate_position
            || self.client_fresh(t)
            || self.minimap_fresh(t)
            || t - self.last_estimate_t < ESTIMATE_EVERY_MS
            || self.last_pos.is_some_and(|(_, lt, _)| t <= lt)
        {
            return out;
        }
        if let Some((p, sigma, v)) = self.edge.predict(t) {
            self.last_estimate_t = t;
            self.emit_player(p, t, &mut out, PosSource::Estimate, Some((sigma, v)));
        }
        out
    }

    fn tick_seen(&mut self, t: Millis) -> Vec<WireEvent> {
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

    /// A frame the client sent to the server: only used for the own
    /// movement, never recorded as a server fact.
    pub fn on_client_frame(&mut self, frame: &Frame, t: Millis) -> Vec<WireEvent> {
        let mut out = Vec::new();
        self.last_t = self.last_t.max(t);
        if let Some(p) = self
            .movement
            .on_client_frame(frame.opcode, &frame.payload, t)
        {
            self.last_client_t = Some(t);
            self.edge.on_truth(p, t, 60.0, false);
            if self.last_pos.is_none_or(|(_, lt, _)| t >= lt) {
                self.emit_player(p, t, &mut out, PosSource::Client, None);
            }
        }
        out
    }

    /// Server clock now, from the heartbeats' timestamps.
    fn server_now(&self, t: Millis) -> Option<i64> {
        self.server_clock.map(|(s, lt)| s + (t - lt))
    }

    pub fn on_frame(&mut self, frame: &Frame, t: Millis) -> Vec<WireEvent> {
        self.started.get_or_insert(t);
        self.last_t = self.last_t.max(t);
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
            // The body is the server's unix time in ms (u64).
            if let Some(ms) = body
                .get(..8)
                .map(|b| i64::from_le_bytes(b.try_into().expect("8 bytes")))
                .filter(|ms| (1_600_000_000_000..2_600_000_000_000).contains(ms))
            {
                self.server_clock = Some((ms, t));
            }
        } else if op == self.ops.self_info {
            if let Some(id) = varint(body, &mut 0) {
                self.self_from_info = true;
                self.set_self(id, &mut out, t);
            }
        } else if op == self.ops.map_load {
            self.edge.quiet(t);
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
            self.edge.quiet(t);
            if let Some(p) = xyz_at(body, 6).filter(|p| is_world_position(*p)) {
                self.own_fix(p, t, &mut out);
            }
        } else if op == self.ops.object_spawn {
            if let Some(s) = parse_spawn(body) {
                self.last_spawn = Some(t);
                if self.objects.len() >= MAX_OBJECTS {
                    self.objects.clear();
                }
                self.objects.insert(s.id, (s.template, s.pos));
                if self.cfg.estimate_position {
                    let age = s.created_ms.zip(self.server_now(t)).map(|(c, now)| now - c);
                    self.edge_event(s.id, s.template, s.pos, Edge::Enter, age, t, &mut out);
                }
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
                let obj = self.objects.remove(&id);
                match (obj, reason) {
                    (Some((template, pos)), 0x01) if self.cfg.estimate_position => {
                        self.edge_event(id, template, pos, Edge::Leave, None, t, &mut out);
                    }
                    _ => self.edge.forget(id),
                }
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
        let Some(&(template, _)) = self.objects.get(&obj) else {
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
            if now - t < 60_000 && self.last_pos.is_none_or(|(_, lt, _)| t >= lt) {
                self.server_fix(p, t, false, out);
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
            self.server_fix(p, t, false, out);
        }
    }

    /// Map load / teleport: always our own position.
    fn own_fix(&mut self, p: [f32; 3], t: Millis, out: &mut Vec<WireEvent>) {
        if let Some(id) = self.self_id {
            self.last_fix_by_entity.insert(id, (p, t));
        }
        self.server_fix(p, t, true, out);
    }

    /// The server told us where we are (`zone`: a map load or teleport).
    fn server_fix(&mut self, p: [f32; 3], t: Millis, zone: bool, out: &mut Vec<WireEvent>) {
        self.self_fix_t = Some(t);
        self.last_fix_pos = Some(p);
        self.movement.on_server_fix(p, t);
        self.minimap.on_fix(t, p, zone);
        self.edge.on_truth(p, t, 150.0, true);
        // Never step back in time: a newer minimap position was already shown.
        if self.last_pos.is_none_or(|(_, lt, _)| t >= lt) {
            let heading = self.camera_heading(t);
            self.emit_player_as(p, t, out, PosSource::Server, None, heading);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn edge_event(
        &mut self,
        id: u64,
        template: u32,
        q: [f32; 3],
        edge: Edge,
        created_age: Option<i64>,
        t: Millis,
        out: &mut Vec<WireEvent>,
    ) {
        let est = self.edge.on_edge(id, template, q, edge, created_age, t);
        if let Some((p, sigma, v)) = est {
            if !self.client_fresh(t)
                && !self.minimap_fresh(t)
                && self.last_pos.is_none_or(|(_, lt, _)| t >= lt)
            {
                self.last_estimate_t = t;
                self.emit_player(p, t, out, PosSource::Estimate, Some((sigma, v)));
            }
        }
    }

    /// Compass heading (degrees) of a game-space direction.
    fn heading_of(&self, dx: f64, dy: f64) -> f64 {
        let north = match self.cfg.north {
            North::NegY => -dy,
            North::PosY => dy,
        };
        dx.atan2(north).to_degrees().rem_euclid(360.0)
    }

    /// `estimate` = (1-sigma, velocity) in game units.
    fn emit_player(
        &mut self,
        p: [f32; 3],
        t: Millis,
        out: &mut Vec<WireEvent>,
        src: PosSource,
        estimate: Option<(f64, (f64, f64))>,
    ) {
        let heading = match estimate {
            Some((_, (vx, vy))) if vx.hypot(vy) > 200.0 => Some(self.heading_of(vx, vy)),
            _ => None,
        };
        let heading = self.camera_heading(t).or(heading);
        self.emit_player_as(p, t, out, src, estimate.map(|(s, _)| s), heading);
    }

    /// `sigma`: 1-sigma uncertainty (game units) of an estimated position;
    /// `heading`: compass degrees, when known (camera direction or travel).
    fn emit_player_as(
        &mut self,
        p: [f32; 3],
        t: Millis,
        out: &mut Vec<WireEvent>,
        src: PosSource,
        sigma: Option<f64>,
        heading: Option<f64>,
    ) {
        // Without a camera direction, the direction of travel is the next
        // best thing, when we are clearly moving (and the previous position
        // came from the same kind of source, so a switch is not a "move").
        let heading = heading.or_else(|| {
            self.last_pos.and_then(|(lp, lt, ls)| {
                let (dx, dy) = ((p[0] - lp[0]) as f64, (p[1] - lp[1]) as f64);
                (ls == src && t > lt && t - lt <= 3_000 && dx.hypot(dy) > 150.0)
                    .then(|| self.heading_of(dx, dy))
            })
        });
        self.last_pos = Some((p, t, src));
        let height_accuracy = match src {
            // Reported (or read) with the position.
            PosSource::Server | PosSource::Client => None,
            PosSource::Minimap | PosSource::Estimate => Some(match self.last_fix_pos {
                Some(f) => {
                    let moved = ((p[0] - f[0]) as f64).hypot((p[1] - f[1]) as f64);
                    HEIGHT_FIX_ERR + HEIGHT_SLOPE * moved
                }
                None => HEIGHT_UNKNOWN,
            }),
        };
        let k = 1.0 / self.cfg.units_per_meter;
        out.push(WireEvent::Player {
            t,
            pos: self.to_world(p),
            heading,
            zone: None,
            accuracy: sigma.map(|s| s * k),
            height_accuracy: height_accuracy.map(|s| s * k),
        });
    }

    pub fn status(&self, now: Millis) -> Aion2Status {
        let mut warnings = Vec::new();
        let since_start = self.started.map(|s| now - s).unwrap_or(0);
        if self.last_heartbeat.is_none_or(|hb| now - hb > 10_000) && since_start > 10_000 {
            warnings.push("No heartbeat from the game for 10 s: is the game running and the right adapter captured?".into());
        }
        if self.self_id.is_none() && since_start > 30_000 {
            warnings.push(
                "Own character not identified yet: open your wings or cast a buff on yourself."
                    .into(),
            );
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
            position_source: match self.last_pos {
                Some((_, _, PosSource::Client)) => "client",
                Some((_, _, PosSource::Server)) => "server",
                Some((_, _, PosSource::Minimap)) => "minimap",
                Some((_, _, PosSource::Estimate)) => "estimate",
                None => "none",
            }
            .into(),
            position_age_ms: self.last_pos.map(|(_, t, _)| now - t),
            minimap: self.minimap.status(now),
            movement: self.movement.status(),
            estimate: self.edge.status(self.cfg.units_per_meter),
            warnings,
        }
    }
}

#[cfg(test)]
mod tests;
