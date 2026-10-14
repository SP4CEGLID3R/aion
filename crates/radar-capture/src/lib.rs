//! Passive capture of the game's network traffic, and the tools to turn it
//! into radar events.
//!
//! ```text
//! live.rs / pcapfile.rs   Npcap/libpcap, or a .pcap/.pcapng file
//!        │ link-layer frames
//! net.rs                  → TCP segments
//! streams.rs              → finds the game connection by its heartbeat,
//!        │                  reassembles it (reassembly.rs)
//! framing.rs              → AION 2 frames (opcode + payload), bundles unpacked
//!        ├─► census.rs    opcode statistics
//!        ├─► discovery.rs markers, correlation, field inspection
//!        ├─► aion2.rs     built-in hidden-cube decoder → radar_ingest::WireEvent
//!        └─► profile.rs   user decoder profiles → radar_ingest::WireEvent
//! source.rs               all of it as a radar DataSource, with recording
//! ```
//!
//! Nothing here sends, injects or modifies traffic, or touches the game
//! process. The built-in decoder implements layouts documented, with
//! evidence, by the aion2-cube-radar project (see `aion2.rs`); everything
//! else comes from profiles checked against recordings.

pub mod aion2;
pub mod census;
pub mod discovery;
pub mod framing;
pub mod live;
pub mod net;
pub mod pcapfile;
pub mod profile;
pub mod reassembly;
pub mod source;
pub mod streams;
pub mod synthetic;

pub use aion2::{Aion2Config, Aion2Status};
pub use profile::DecoderProfile;
pub use source::{
    add_marker, CaptureConfig, CaptureInput, CaptureSource, CaptureStatus, DecoderChoice,
    SharedStatus,
};
