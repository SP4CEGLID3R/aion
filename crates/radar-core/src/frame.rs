//! What the renderer receives: a read-only snapshot of what may be drawn.

use serde::{Deserialize, Serialize};

use crate::geometry::WorldPos;
use crate::model::{CubeState, Millis};

/// How cubes whose state can no longer be verified are presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StaleDisplay {
    /// Stale cubes are removed from the radar (default).
    #[default]
    Hide,
    /// Stale cubes are drawn hollow, greyed, and labelled STALE.
    MarkStale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerView {
    pub pos: WorldPos,
    pub heading_deg: Option<f64>,
    pub age_ms: Millis,
    /// The player position is older than the stale timeout.
    pub stale: bool,
    /// 1-sigma uncertainty in metres when the position is an estimate.
    pub accuracy_m: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CubeView {
    pub id: String,
    pub label: Option<String>,
    /// `Spawned`, or `Stale` when stale cubes are shown. Nothing else.
    pub state: CubeState,
    pub pos: WorldPos,
    /// Horizontal distance from the player; `None` without a player position.
    pub distance: Option<f64>,
    /// Compass bearing from the player; `None` without a player position.
    pub bearing_deg: Option<f64>,
    /// Time since the cube was last confirmed present.
    pub confirmed_age_ms: Millis,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameStats {
    pub spawned: usize,
    pub stale: usize,
    pub tracked: usize,
    pub accepted: u64,
    pub duplicates: u64,
    pub rejected: u64,
}

/// Health of the data source feeding the radar, filled in by the pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceHealth {
    pub name: String,
    pub state: SourceState,
    pub detail: String,
    pub last_event_at: Option<Millis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Starting,
    Connected,
    Waiting,
    Finished,
    Error,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadarFrame {
    pub generated_at: Millis,
    pub zone: Option<String>,
    pub player: Option<PlayerView>,
    pub cubes: Vec<CubeView>,
    pub stats: FrameStats,
    pub source: Option<SourceHealth>,
}
