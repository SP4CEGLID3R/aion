//! Getting events into the radar.
//!
//! ```text
//! DataSource (thread)          simulator | replay file | 127.0.0.1 TCP
//!     │ WireEvent              Radar Event Protocol v1 (wire.rs)
//!     ▼                        (srf.rs: synthetic byte-stream parser → WireEvent)
//! EventAdapter                 source frame → radar frame, provenance tagging
//!     │ RadarEvent
//!     ▼
//! radar_core::Tracker          live-cube state machine
//!     │ RadarFrame
//!     ▼
//! FrameSink                    the Tauri app (or a test)
//! ```

pub mod adapter;
pub mod clock;
pub mod pipeline;
pub mod simulator;
pub mod source;
pub mod srf;
pub mod wire;

pub use adapter::{AdaptError, EventAdapter};
pub use pipeline::{DebugReport, Engine, EngineError, FrameSink};
pub use source::{DataSource, SourceMessage};
pub use wire::{parse_line, WireEvent, WireId, WireProvenance};
