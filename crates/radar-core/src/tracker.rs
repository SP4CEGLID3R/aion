//! Live cube state: the single place that decides what counts as "currently
//! spawned".
//!
//! Rules, in order of precedence:
//!
//! 1. Only [`Provenance::Live`] observations change state. Static spawn
//!    points, historical and predicted locations are rejected and only logged.
//! 2. A cube needs a stable id. Without one, its despawn could never be
//!    matched, so it is rejected rather than shown until it times out.
//! 3. A marker appears only on a confirmed `Spawned` event with a valid
//!    position. A `Seen` for a cube never confirmed spawned leaves it
//!    `Unknown`, which is never drawn.
//! 4. `Despawned`/`Collected` remove the marker and leave a tombstone, so a
//!    late event older than the removal cannot resurrect it.
//! 5. A spawned cube not re-confirmed within the stale timeout becomes
//!    `Stale`; [`Tracker::frame`] applies the same age test itself, so a
//!    missed [`Tracker::tick`] cannot make an old cube look live.
//! 6. Events are per-id ordered by their own timestamp; anything older than
//!    the last applied event for that id is rejected as out of order.
//! 7. A zone change drops every cube: ids are not assumed to survive it.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::frame::{CubeView, FrameStats, PlayerView, RadarFrame, StaleDisplay};
use crate::geometry::WorldPos;
use crate::model::{
    CubeEvent, CubeEventKind, CubeState, Millis, PlayerUpdate, Provenance, RadarEvent,
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerConfig {
    /// A spawned cube (or the player) unconfirmed for longer than this is stale.
    pub stale_timeout_ms: Millis,
    /// How long a stale cube is kept (and optionally drawn as stale) before
    /// it is forgotten.
    pub stale_retain_ms: Millis,
    /// How long despawn tombstones are kept to block out-of-order events.
    pub tombstone_ttl_ms: Millis,
    /// Events stamped further than this into the future are rejected.
    pub max_clock_skew_ms: Millis,
    /// Number of debug history entries kept.
    pub history_capacity: usize,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            stale_timeout_ms: 10_000,
            stale_retain_ms: 30_000,
            tombstone_ttl_ms: 120_000,
            max_clock_skew_ms: 2_000,
            history_capacity: 500,
        }
    }
}

/// Why an event was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    NotLive,
    MissingId,
    MissingPosition,
    NonFinitePosition,
    FutureTimestamp,
    TooOld,
    OutOfOrder,
    WrongZone,
}

impl RejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotLive => "not_live",
            Self::MissingId => "missing_id",
            Self::MissingPosition => "missing_position",
            Self::NonFinitePosition => "non_finite_position",
            Self::FutureTimestamp => "future_timestamp",
            Self::TooOld => "too_old",
            Self::OutOfOrder => "out_of_order",
            Self::WrongZone => "wrong_zone",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The event changed state.
    Applied,
    /// The event restated what was already known (it may refresh timers).
    Duplicate,
    Rejected(RejectReason),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeRecord {
    pub id: String,
    pub state: CubeState,
    pub pos: Option<WorldPos>,
    pub label: Option<String>,
    pub zone: Option<String>,
    /// Source time of the last event that confirmed the cube present.
    pub last_confirmed_at: Option<Millis>,
    /// Source time of the last applied event of any kind.
    pub last_event_at: Millis,
    /// Local time of the last state change.
    pub state_since: Millis,
}

/// One debug log line. History is for diagnostics only and is never rendered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub at: Millis,
    pub observed_at: Millis,
    pub event: String,
    pub id: Option<String>,
    pub provenance: Option<Provenance>,
    pub pos: Option<WorldPos>,
    pub outcome: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub accepted: u64,
    pub duplicates: u64,
    pub rejected: BTreeMap<String, u64>,
    /// Rejections of non-live provenance, by provenance.
    pub not_live_by_provenance: BTreeMap<String, u64>,
}

impl Diagnostics {
    pub fn rejected_total(&self) -> u64 {
        self.rejected.values().sum()
    }
}

#[derive(Debug, Clone)]
pub struct Tracker {
    config: TrackerConfig,
    zone: Option<String>,
    player: Option<PlayerUpdate>,
    cubes: BTreeMap<String, CubeRecord>,
    history: VecDeque<HistoryEntry>,
    diagnostics: Diagnostics,
}

impl Tracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self {
            config,
            zone: None,
            player: None,
            cubes: BTreeMap::new(),
            history: VecDeque::new(),
            diagnostics: Diagnostics::default(),
        }
    }

    pub fn config(&self) -> &TrackerConfig {
        &self.config
    }

    pub fn zone(&self) -> Option<&str> {
        self.zone.as_deref()
    }

    pub fn cube(&self, id: &str) -> Option<&CubeRecord> {
        self.cubes.get(id)
    }

    pub fn cubes(&self) -> impl Iterator<Item = &CubeRecord> {
        self.cubes.values()
    }

    pub fn history(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.history.iter()
    }

    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// Applies one event. `now` is the local clock in the same epoch as the
    /// events' `observed_at`.
    pub fn apply(&mut self, event: RadarEvent, now: Millis) -> Outcome {
        let outcome = match &event {
            RadarEvent::Player(update) => self.apply_player(update, now),
            RadarEvent::Cube(cube) => self.apply_cube(cube, now),
            RadarEvent::ZoneChanged { zone, observed_at } => {
                if *observed_at > now + self.config.max_clock_skew_ms {
                    Outcome::Rejected(RejectReason::FutureTimestamp)
                } else {
                    self.change_zone(zone.clone(), now);
                    Outcome::Applied
                }
            }
        };
        self.count(&event, outcome);
        // Player updates arrive many times a second; logging them would push
        // every cube event out of the history.
        if !matches!(event, RadarEvent::Player(_)) {
            self.log(&event, outcome, now);
        }
        outcome
    }

    fn apply_player(&mut self, update: &PlayerUpdate, now: Millis) -> Outcome {
        if !update.pos.is_finite() || update.heading_deg.is_some_and(|h| !h.is_finite()) {
            return Outcome::Rejected(RejectReason::NonFinitePosition);
        }
        if update.observed_at > now + self.config.max_clock_skew_ms {
            return Outcome::Rejected(RejectReason::FutureTimestamp);
        }
        if let Some(prev) = &self.player {
            if update.observed_at < prev.observed_at {
                return Outcome::Rejected(RejectReason::OutOfOrder);
            }
        }
        if let Some(zone) = &update.zone {
            match &self.zone {
                Some(current) if current != zone => self.change_zone(zone.clone(), now),
                None => self.zone = Some(zone.clone()),
                _ => {}
            }
        }
        self.player = Some(update.clone());
        Outcome::Applied
    }

    fn apply_cube(&mut self, ev: &CubeEvent, now: Millis) -> Outcome {
        if ev.provenance != Provenance::Live {
            return Outcome::Rejected(RejectReason::NotLive);
        }
        let Some(id) = ev.id.as_deref().filter(|id| !id.is_empty()) else {
            return Outcome::Rejected(RejectReason::MissingId);
        };
        if ev.observed_at > now + self.config.max_clock_skew_ms {
            return Outcome::Rejected(RejectReason::FutureTimestamp);
        }
        if let (Some(ev_zone), Some(zone)) = (&ev.zone, &self.zone) {
            if ev_zone != zone {
                return Outcome::Rejected(RejectReason::WrongZone);
            }
        }
        if let Some(pos) = &ev.pos {
            if !pos.is_finite() {
                return Outcome::Rejected(RejectReason::NonFinitePosition);
            }
        }
        if let Some(rec) = self.cubes.get(id) {
            if ev.observed_at < rec.last_event_at {
                return Outcome::Rejected(RejectReason::OutOfOrder);
            }
        }

        let confirms_presence = matches!(ev.kind, CubeEventKind::Spawned | CubeEventKind::Seen);
        if confirms_presence && now - ev.observed_at > self.config.stale_timeout_ms {
            // Already too old to vouch for the present.
            return Outcome::Rejected(RejectReason::TooOld);
        }

        match ev.kind {
            CubeEventKind::Spawned => self.on_spawned(id, ev, now),
            CubeEventKind::Seen => self.on_seen(id, ev, now),
            CubeEventKind::Despawned => self.on_gone(id, ev, CubeState::Despawned, now),
            CubeEventKind::Collected => self.on_gone(id, ev, CubeState::Collected, now),
            CubeEventKind::Lost => self.on_lost(id, ev, now),
        }
    }

    fn on_lost(&mut self, id: &str, ev: &CubeEvent, now: Millis) -> Outcome {
        match self.cubes.get_mut(id) {
            Some(rec) if rec.state == CubeState::Spawned => {
                rec.state = CubeState::Stale;
                rec.last_event_at = ev.observed_at;
                rec.state_since = now;
                Outcome::Applied
            }
            Some(rec) => {
                rec.last_event_at = ev.observed_at;
                Outcome::Duplicate
            }
            // Losing sight of something never seen changes nothing.
            None => Outcome::Duplicate,
        }
    }

    fn on_spawned(&mut self, id: &str, ev: &CubeEvent, now: Millis) -> Outcome {
        let Some(pos) = ev.pos else {
            return Outcome::Rejected(RejectReason::MissingPosition);
        };
        let zone = ev.zone.clone().or_else(|| self.zone.clone());
        match self.cubes.get_mut(id) {
            Some(rec) if rec.state == CubeState::Spawned => {
                rec.last_event_at = ev.observed_at;
                rec.last_confirmed_at = Some(ev.observed_at);
                rec.pos = Some(pos);
                if ev.label.is_some() {
                    rec.label = ev.label.clone();
                }
                Outcome::Duplicate
            }
            _ => {
                self.cubes.insert(
                    id.to_owned(),
                    CubeRecord {
                        id: id.to_owned(),
                        state: CubeState::Spawned,
                        pos: Some(pos),
                        label: ev.label.clone(),
                        zone,
                        last_confirmed_at: Some(ev.observed_at),
                        last_event_at: ev.observed_at,
                        state_since: now,
                    },
                );
                Outcome::Applied
            }
        }
    }

    fn on_seen(&mut self, id: &str, ev: &CubeEvent, now: Millis) -> Outcome {
        match self.cubes.get_mut(id) {
            Some(rec) => match rec.state {
                CubeState::Spawned | CubeState::Stale => {
                    let revived = rec.state == CubeState::Stale;
                    rec.last_event_at = ev.observed_at;
                    rec.last_confirmed_at = Some(ev.observed_at);
                    if let Some(pos) = ev.pos {
                        rec.pos = Some(pos);
                    }
                    if revived {
                        rec.state = CubeState::Spawned;
                        rec.state_since = now;
                        Outcome::Applied
                    } else {
                        Outcome::Duplicate
                    }
                }
                CubeState::Unknown => {
                    rec.last_event_at = ev.observed_at;
                    if ev.pos.is_some() {
                        rec.pos = ev.pos;
                    }
                    Outcome::Duplicate
                }
                // Seen after a confirmed removal, with a newer timestamp: the
                // source contradicts itself. Without a new spawn, do not guess.
                CubeState::Despawned | CubeState::Collected => {
                    rec.last_event_at = ev.observed_at;
                    rec.state = CubeState::Unknown;
                    rec.state_since = now;
                    rec.pos = ev.pos;
                    Outcome::Applied
                }
            },
            None => {
                self.cubes.insert(
                    id.to_owned(),
                    CubeRecord {
                        id: id.to_owned(),
                        state: CubeState::Unknown,
                        pos: ev.pos,
                        label: ev.label.clone(),
                        zone: ev.zone.clone().or_else(|| self.zone.clone()),
                        last_confirmed_at: None,
                        last_event_at: ev.observed_at,
                        state_since: now,
                    },
                );
                Outcome::Applied
            }
        }
    }

    fn on_gone(&mut self, id: &str, ev: &CubeEvent, state: CubeState, now: Millis) -> Outcome {
        match self.cubes.get_mut(id) {
            Some(rec) if rec.state == state => {
                rec.last_event_at = ev.observed_at;
                Outcome::Duplicate
            }
            Some(rec) => {
                rec.state = state;
                rec.last_event_at = ev.observed_at;
                rec.state_since = now;
                Outcome::Applied
            }
            None => {
                // A removal for a cube we never saw: keep a tombstone so an
                // older, late spawn for the same id cannot appear afterwards.
                self.cubes.insert(
                    id.to_owned(),
                    CubeRecord {
                        id: id.to_owned(),
                        state,
                        pos: None,
                        label: None,
                        zone: ev.zone.clone().or_else(|| self.zone.clone()),
                        last_confirmed_at: None,
                        last_event_at: ev.observed_at,
                        state_since: now,
                    },
                );
                Outcome::Applied
            }
        }
    }

    fn change_zone(&mut self, zone: String, now: Millis) {
        let dropped = self.cubes.len();
        self.cubes.clear();
        self.player = None;
        self.history_push(HistoryEntry {
            at: now,
            observed_at: now,
            event: "zone_reset".into(),
            id: None,
            provenance: None,
            pos: None,
            outcome: format!("dropped {dropped} tracked cubes; entering {zone}"),
        });
        self.zone = Some(zone);
    }

    /// Advances timers: spawned → stale, and forgets expired records.
    pub fn tick(&mut self, now: Millis) {
        let cfg = self.config;
        let mut expired = Vec::new();
        let mut went_stale = Vec::new();
        for rec in self.cubes.values_mut() {
            match rec.state {
                CubeState::Spawned => {
                    let confirmed = rec.last_confirmed_at.unwrap_or(rec.last_event_at);
                    if now - confirmed > cfg.stale_timeout_ms {
                        rec.state = CubeState::Stale;
                        rec.state_since = now;
                        went_stale.push(rec.id.clone());
                    }
                }
                CubeState::Stale => {
                    if now - rec.state_since > cfg.stale_retain_ms {
                        expired.push(rec.id.clone());
                    }
                }
                CubeState::Unknown => {
                    if now - rec.state_since > cfg.stale_timeout_ms {
                        expired.push(rec.id.clone());
                    }
                }
                CubeState::Despawned | CubeState::Collected => {
                    if now - rec.state_since > cfg.tombstone_ttl_ms {
                        expired.push(rec.id.clone());
                    }
                }
            }
        }
        for id in went_stale {
            self.history_push(HistoryEntry {
                at: now,
                observed_at: now,
                event: "timeout".into(),
                id: Some(id),
                provenance: None,
                pos: None,
                outcome: "stale".into(),
            });
        }
        for id in expired {
            self.cubes.remove(&id);
        }
    }

    /// Builds the render snapshot. Only cubes confirmed present within the
    /// stale timeout are reported as `Spawned`; stale ones are included only
    /// when asked for, and are always labelled `Stale`.
    pub fn frame(&self, now: Millis, stale_display: StaleDisplay) -> RadarFrame {
        let timeout = self.config.stale_timeout_ms;
        let player = self.player.as_ref().map(|p| PlayerView {
            pos: p.pos,
            heading_deg: p.heading_deg,
            age_ms: now - p.observed_at,
            stale: now - p.observed_at > timeout,
            accuracy_m: p.accuracy_m,
            height_accuracy_m: p.height_accuracy_m,
        });

        let mut stats = FrameStats {
            tracked: self.cubes.len(),
            accepted: self.diagnostics.accepted,
            duplicates: self.diagnostics.duplicates,
            rejected: self.diagnostics.rejected_total(),
            ..Default::default()
        };

        let mut cubes = Vec::new();
        for rec in self.cubes.values() {
            let Some(pos) = rec.pos else { continue };
            let confirmed_age = rec.last_confirmed_at.map(|t| now - t);
            let state = match (rec.state, confirmed_age) {
                (CubeState::Spawned, Some(age)) if age <= timeout => CubeState::Spawned,
                (CubeState::Spawned, Some(_)) | (CubeState::Stale, Some(_)) => CubeState::Stale,
                _ => continue,
            };
            match state {
                CubeState::Spawned => stats.spawned += 1,
                _ => stats.stale += 1,
            }
            if state == CubeState::Stale && stale_display == StaleDisplay::Hide {
                continue;
            }
            let (distance, bearing_deg, height) = match &player {
                Some(p) => {
                    let off = p.pos.offset_to(&pos);
                    (
                        Some(off.horizontal_distance()),
                        off.bearing_deg(),
                        Some(off.up),
                    )
                }
                None => (None, None, None),
            };
            cubes.push(CubeView {
                id: rec.id.clone(),
                label: rec.label.clone(),
                state,
                pos,
                distance,
                bearing_deg,
                height,
                confirmed_age_ms: confirmed_age.unwrap_or_default(),
            });
        }

        RadarFrame {
            generated_at: now,
            zone: self.zone.clone(),
            player,
            cubes,
            stats,
            source: None,
        }
    }

    fn count(&mut self, event: &RadarEvent, outcome: Outcome) {
        match outcome {
            Outcome::Applied => self.diagnostics.accepted += 1,
            Outcome::Duplicate => self.diagnostics.duplicates += 1,
            Outcome::Rejected(reason) => {
                *self
                    .diagnostics
                    .rejected
                    .entry(reason.as_str().to_owned())
                    .or_default() += 1;
                if let (RejectReason::NotLive, RadarEvent::Cube(c)) = (reason, event) {
                    *self
                        .diagnostics
                        .not_live_by_provenance
                        .entry(c.provenance.as_str().to_owned())
                        .or_default() += 1;
                }
            }
        }
    }

    fn log(&mut self, event: &RadarEvent, outcome: Outcome, now: Millis) {
        let outcome = match outcome {
            Outcome::Applied => "applied".to_owned(),
            Outcome::Duplicate => "duplicate".to_owned(),
            Outcome::Rejected(r) => format!("rejected:{}", r.as_str()),
        };
        let entry = match event {
            RadarEvent::Cube(c) => HistoryEntry {
                at: now,
                observed_at: c.observed_at,
                event: format!("cube_{:?}", c.kind).to_lowercase(),
                id: c.id.clone(),
                provenance: Some(c.provenance),
                pos: c.pos,
                outcome,
            },
            RadarEvent::ZoneChanged { zone, observed_at } => HistoryEntry {
                at: now,
                observed_at: *observed_at,
                event: "zone_changed".into(),
                id: Some(zone.clone()),
                provenance: None,
                pos: None,
                outcome,
            },
            RadarEvent::Player(_) => return,
        };
        self.history_push(entry);
    }

    fn history_push(&mut self, entry: HistoryEntry) {
        if self.config.history_capacity == 0 {
            return;
        }
        while self.history.len() >= self.config.history_capacity {
            self.history.pop_front();
        }
        self.history.push_back(entry);
    }
}

#[cfg(test)]
mod tests;
