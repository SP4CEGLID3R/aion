//! The radar's internal event model.
//!
//! Every data source, whatever it speaks, is converted into these types by an
//! adapter before it reaches the [`Tracker`](crate::tracker::Tracker). They
//! carry *claims* about the world; the tracker decides which claims are good
//! enough to show.

use serde::{Deserialize, Serialize};

use crate::geometry::WorldPos;

/// Milliseconds since the Unix epoch. Signed so that ages can be subtracted
/// without wrapping.
pub type Millis = i64;

/// Where a cube observation comes from. Only [`Provenance::Live`] can ever put
/// a marker on the radar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Observed now by a validated, authorized source.
    #[default]
    Live,
    /// A past observation (a log, a replayed history entry).
    Historical,
    /// Reference data: a place where cubes *can* spawn.
    StaticSpawnPoint,
    /// Inferred or predicted, not observed.
    Predicted,
}

impl Provenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Historical => "historical",
            Self::StaticSpawnPoint => "static_spawn_point",
            Self::Predicted => "predicted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CubeEventKind {
    /// The source confirms the cube has spawned at `pos`.
    Spawned,
    /// The source re-confirms an already-known cube is still present.
    Seen,
    /// The source confirms the cube is gone.
    Despawned,
    /// The source confirms the cube was collected (gone, with a reason).
    Collected,
    /// The source can no longer vouch for the cube (e.g. it left the range
    /// the server reports). It may still exist, so it is not "gone", but it
    /// is no longer verified: it becomes stale at once.
    Lost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CubeEvent {
    /// Stable identifier from the source. Required for anything to be shown:
    /// without it a later despawn cannot be matched to this cube.
    pub id: Option<String>,
    pub kind: CubeEventKind,
    pub pos: Option<WorldPos>,
    pub label: Option<String>,
    pub zone: Option<String>,
    pub provenance: Provenance,
    pub observed_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerUpdate {
    pub pos: WorldPos,
    /// Compass heading in degrees, when the source knows it.
    pub heading_deg: Option<f64>,
    pub zone: Option<String>,
    pub observed_at: Millis,
    /// 1-sigma uncertainty in metres when the position is estimated.
    #[serde(default)]
    pub accuracy_m: Option<f64>,
    /// 1-sigma uncertainty of the height in metres, when it was not measured
    /// with this position.
    #[serde(default)]
    pub height_accuracy_m: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RadarEvent {
    Player(PlayerUpdate),
    Cube(CubeEvent),
    /// The player moved to another zone/instance. Entity ids are not assumed
    /// to survive this, so every tracked cube is dropped.
    ZoneChanged {
        zone: String,
        observed_at: Millis,
    },
}

/// Lifecycle of a tracked cube. Only `Spawned` means "currently present".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CubeState {
    /// Known to the source but never confirmed as spawned (e.g. a "seen"
    /// without a spawn). Never rendered.
    Unknown,
    /// Confirmed spawned and confirmed recently enough to be trusted.
    Spawned,
    /// Confirmed gone. Kept briefly as a tombstone so a late, older event
    /// cannot bring it back. Never rendered.
    Despawned,
    /// Confirmed collected. Same handling as `Despawned`.
    Collected,
    /// Was spawned, but no confirmation arrived within the stale timeout.
    /// Hidden by default; can be drawn as clearly-marked stale.
    Stale,
}
