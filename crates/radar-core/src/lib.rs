//! Core of the AION 2 radar overlay: the event model, the live-cube state
//! machine, coordinate maths, and settings.
//!
//! Nothing here does I/O, spawns threads, or knows about Tauri. Time is always
//! passed in explicitly, so every rule can be tested deterministically.

pub mod frame;
pub mod geometry;
pub mod model;
pub mod settings;
pub mod tracker;

pub use frame::{
    CubeView, FrameStats, PlayerView, RadarFrame, SourceHealth, SourceState, StaleDisplay,
};
pub use geometry::{CoordinateTransform, Offset, SourceAxis, TransformError, WorldPos};
pub use model::{
    CubeEvent, CubeEventKind, CubeState, Millis, PlayerUpdate, Provenance, RadarEvent,
};
pub use settings::{DataSourceConfig, RadarColors, RadarSettings, RotationMode, WindowMode};
pub use tracker::{
    CubeRecord, Diagnostics, HistoryEntry, Outcome, RejectReason, Tracker, TrackerConfig,
};
