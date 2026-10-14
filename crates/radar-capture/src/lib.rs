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
//!        └─► profile.rs   decoder profile → radar_ingest::WireEvent
//! source.rs               all of it as a radar DataSource, with recording
//! ```
//!
//! Nothing here sends, injects or modifies traffic, or touches the game
//! process. No packet layout for cubes is built in: it comes from a decoder
//! profile that is found with the discovery tools and validated by a person.

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

pub use profile::DecoderProfile;
pub use source::{
    add_marker, CaptureConfig, CaptureInput, CaptureSource, CaptureStatus, SharedStatus,
};
