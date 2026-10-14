//! Coordinate frames and the maths between them.
//!
//! The radar's world frame is fixed: `x` = east, `y` = north, `z` = up, in
//! "world units". A data source may use any axis order, handedness, scale and
//! origin; [`CoordinateTransform`] maps it into the radar frame once, at the
//! adapter, so nothing downstream needs to know how the source counts.
//!
//! Headings and bearings are compass degrees: 0 = north, 90 = east, clockwise,
//! normalised to `[0, 360)`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A point in the radar world frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorldPos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl WorldPos {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// The offset from `self` to `other`, expressed as east/north/up.
    pub fn offset_to(&self, other: &WorldPos) -> Offset {
        Offset {
            east: other.x - self.x,
            north: other.y - self.y,
            up: other.z - self.z,
        }
    }
}

/// A displacement in the radar world frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Offset {
    pub east: f64,
    pub north: f64,
    pub up: f64,
}

impl Offset {
    /// Distance on the ground plane, ignoring height.
    pub fn horizontal_distance(&self) -> f64 {
        self.east.hypot(self.north)
    }

    pub fn distance(&self) -> f64 {
        (self.east * self.east + self.north * self.north + self.up * self.up).sqrt()
    }

    /// Compass bearing of this offset. `None` when it has no horizontal
    /// component, because a direction to "right here" is meaningless.
    pub fn bearing_deg(&self) -> Option<f64> {
        if self.horizontal_distance() < 1e-9 {
            return None;
        }
        Some(normalize_deg(self.east.atan2(self.north).to_degrees()))
    }
}

/// Normalises an angle in degrees to `[0, 360)`.
pub fn normalize_deg(deg: f64) -> f64 {
    let r = deg.rem_euclid(360.0);
    // rem_euclid can return 360.0 for tiny negative inputs through rounding.
    if r >= 360.0 {
        0.0
    } else {
        r
    }
}

/// One signed axis of a source's coordinate frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAxis {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl SourceAxis {
    fn index(self) -> usize {
        match self {
            Self::PosX | Self::NegX => 0,
            Self::PosY | Self::NegY => 1,
            Self::PosZ | Self::NegZ => 2,
        }
    }

    fn sign(self) -> f64 {
        match self {
            Self::PosX | Self::PosY | Self::PosZ => 1.0,
            Self::NegX | Self::NegY | Self::NegZ => -1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum TransformError {
    #[error("east, north and up must each use a different source axis")]
    RepeatedAxis,
    #[error("scale must be a finite number greater than zero, got {0}")]
    BadScale(f64),
    #[error("origin and heading offset must be finite numbers")]
    NonFinite,
}

/// Maps source coordinates and headings into the radar world frame.
///
/// `world.east = sign(east) * (src[east] - origin[east]) * scale`, and the same
/// for north and up. Reflections are allowed on purpose: a source with a
/// left-handed frame or a Y-down map is fixed by flipping one axis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CoordinateTransform {
    /// Which source axis points east.
    pub east: SourceAxis,
    /// Which source axis points north.
    pub north: SourceAxis,
    /// Which source axis points up.
    pub up: SourceAxis,
    /// World units per source unit.
    pub scale: f64,
    /// The source-space point that maps to the world origin.
    pub origin: [f64; 3],
    /// Added to the source heading after direction correction.
    pub heading_offset_deg: f64,
    /// Whether the source's heading increases clockwise (compass style).
    pub heading_clockwise: bool,
}

impl Default for CoordinateTransform {
    fn default() -> Self {
        Self {
            east: SourceAxis::PosX,
            north: SourceAxis::PosY,
            up: SourceAxis::PosZ,
            scale: 1.0,
            origin: [0.0; 3],
            heading_offset_deg: 0.0,
            heading_clockwise: true,
        }
    }
}

impl CoordinateTransform {
    pub fn validate(&self) -> Result<(), TransformError> {
        let (e, n, u) = (self.east.index(), self.north.index(), self.up.index());
        if e == n || n == u || e == u {
            return Err(TransformError::RepeatedAxis);
        }
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return Err(TransformError::BadScale(self.scale));
        }
        if !self.origin.iter().all(|v| v.is_finite()) || !self.heading_offset_deg.is_finite() {
            return Err(TransformError::NonFinite);
        }
        Ok(())
    }

    /// Maps a source-space point to the world frame. The transform is assumed
    /// valid; call [`validate`](Self::validate) when it is configured.
    pub fn apply(&self, src: [f64; 3]) -> WorldPos {
        let d = [
            src[0] - self.origin[0],
            src[1] - self.origin[1],
            src[2] - self.origin[2],
        ];
        let pick = |axis: SourceAxis| axis.sign() * d[axis.index()] * self.scale;
        WorldPos::new(pick(self.east), pick(self.north), pick(self.up))
    }

    /// Maps a source heading to a compass heading.
    pub fn apply_heading(&self, deg: f64) -> f64 {
        let h = if self.heading_clockwise { deg } else { -deg };
        normalize_deg(h + self.heading_offset_deg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn bearing_follows_the_compass() {
        let o = WorldPos::new(0.0, 0.0, 0.0);
        let cases = [
            ((0.0, 10.0), 0.0),
            ((10.0, 0.0), 90.0),
            ((0.0, -10.0), 180.0),
            ((-10.0, 0.0), 270.0),
            ((10.0, 10.0), 45.0),
            ((-10.0, 10.0), 315.0),
        ];
        for ((x, y), expected) in cases {
            let b = o
                .offset_to(&WorldPos::new(x, y, 0.0))
                .bearing_deg()
                .unwrap();
            assert!(close(b, expected), "({x},{y}) -> {b}, expected {expected}");
        }
    }

    #[test]
    fn bearing_is_undefined_on_top_of_the_player() {
        let o = WorldPos::new(5.0, 5.0, 0.0);
        assert_eq!(
            o.offset_to(&WorldPos::new(5.0, 5.0, 30.0)).bearing_deg(),
            None
        );
    }

    #[test]
    fn distances_separate_ground_and_height() {
        let off = WorldPos::new(1.0, 1.0, 1.0).offset_to(&WorldPos::new(4.0, 5.0, 13.0));
        assert!(close(off.horizontal_distance(), 5.0));
        assert!(close(off.distance(), 13.0));
    }

    #[test]
    fn normalize_handles_negative_and_large_angles() {
        assert!(close(normalize_deg(-90.0), 270.0));
        assert!(close(normalize_deg(720.0), 0.0));
        let tiny = normalize_deg(-1e-15);
        assert!((0.0..360.0).contains(&tiny));
    }

    #[test]
    fn identity_transform_changes_nothing() {
        let t = CoordinateTransform::default();
        t.validate().unwrap();
        assert_eq!(t.apply([1.0, 2.0, 3.0]), WorldPos::new(1.0, 2.0, 3.0));
        assert!(close(t.apply_heading(370.0), 10.0));
    }

    #[test]
    fn y_up_engine_frame_maps_z_to_north() {
        // A common game-engine layout: X east, Y up, Z north.
        let t = CoordinateTransform {
            east: SourceAxis::PosX,
            north: SourceAxis::PosZ,
            up: SourceAxis::PosY,
            ..Default::default()
        };
        t.validate().unwrap();
        assert_eq!(t.apply([1.0, 2.0, 3.0]), WorldPos::new(1.0, 3.0, 2.0));
    }

    #[test]
    fn image_space_y_down_is_flipped_and_scaled_around_an_origin() {
        let t = CoordinateTransform {
            north: SourceAxis::NegY,
            scale: 0.5,
            origin: [100.0, 100.0, 0.0],
            ..Default::default()
        };
        t.validate().unwrap();
        // 20 px right of and 40 px below the origin in image space.
        assert_eq!(
            t.apply([120.0, 140.0, 0.0]),
            WorldPos::new(10.0, -20.0, 0.0)
        );
    }

    #[test]
    fn counter_clockwise_heading_is_converted() {
        let t = CoordinateTransform {
            heading_clockwise: false,
            heading_offset_deg: 90.0,
            ..Default::default()
        };
        // Math-style angle: 0 = east, counter-clockwise. 90 (north) -> compass 0.
        assert!(close(t.apply_heading(90.0), 0.0));
        assert!(close(t.apply_heading(0.0), 90.0));
    }

    #[test]
    fn invalid_transforms_are_rejected() {
        let repeated = CoordinateTransform {
            north: SourceAxis::NegX,
            ..Default::default()
        };
        assert_eq!(repeated.validate(), Err(TransformError::RepeatedAxis));

        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let t = CoordinateTransform {
                scale,
                ..Default::default()
            };
            assert!(matches!(t.validate(), Err(TransformError::BadScale(_))));
        }

        let origin = CoordinateTransform {
            origin: [f64::NAN, 0.0, 0.0],
            ..Default::default()
        };
        assert_eq!(origin.validate(), Err(TransformError::NonFinite));
    }

    #[test]
    fn transform_round_trips_through_json() {
        let t = CoordinateTransform {
            north: SourceAxis::NegZ,
            up: SourceAxis::PosY,
            scale: 0.01,
            ..Default::default()
        };
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"north\":\"neg_z\""));
        let back: CoordinateTransform = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }
}
