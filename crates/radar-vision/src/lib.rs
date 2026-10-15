//! Reads the AION 2 minimap from the screen to follow the player between the
//! server's own-position reports.
//!
//! The minimap (top right) is north-up and centred on the player: as you
//! move, the map scrolls under a fixed arrow. Measuring that scroll frame by
//! frame gives your movement in minimap pixels ([`tracker`]); the radar's
//! decoder turns it into world units with the server's position fixes. The
//! view cone on the minimap shows the camera direction.
//!
//! Passive only: a plain screen copy of one rectangle ([`capture`]); nothing
//! touches the game process, its memory or its traffic.
//!
//! The approach and its constants (masks, band-pass, masked NCC with
//! keyframes, cone detection) follow madara134/aion2-cube-finder, MIT
//! licensed — see `NOTICE` in this crate. This is an independent Rust
//! implementation.

pub mod capture;
pub mod locate;
pub mod ncc;
pub mod overlay;
pub mod plane;
pub mod synth;
pub mod tracker;

pub use capture::{CaptureError, RegionGrabber, ScreenRect};
pub use plane::Bgra;
pub use tracker::{choose_downsample, MinimapTracker, Step, TrackerConfig, TrackerStats, RIDGE};

#[cfg(test)]
mod tests;
