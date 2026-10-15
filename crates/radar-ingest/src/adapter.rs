//! Converts Radar Event Protocol events into the radar's internal model.
//!
//! This is the only place that knows about the source's coordinate frame.
//! It also makes provenance explicit: a spawn point or historical location is
//! converted into a cube event *tagged* as such, so the tracker rejects it
//! and the rejection is counted, rather than it silently vanishing here.

use radar_core::{
    CoordinateTransform, CubeEvent, CubeEventKind, Millis, PlayerUpdate, Provenance, RadarEvent,
    TransformError,
};
use thiserror::Error;

use crate::wire::{WireEvent, WireId, WireProvenance};

#[derive(Debug, Error, PartialEq)]
pub enum AdaptError {
    #[error("coordinates are not finite numbers")]
    NonFinite,
    #[error("zone name is empty")]
    EmptyZone,
}

#[derive(Debug, Clone)]
pub struct EventAdapter {
    transform: CoordinateTransform,
}

impl EventAdapter {
    pub fn new(transform: CoordinateTransform) -> Result<Self, TransformError> {
        transform.validate()?;
        Ok(Self { transform })
    }

    pub fn convert(&self, event: WireEvent) -> Result<RadarEvent, AdaptError> {
        Ok(match event {
            WireEvent::Player {
                t,
                pos,
                heading,
                zone,
                accuracy,
                height_accuracy,
            } => {
                let metres = |a: Option<f64>| match a {
                    Some(a) if !a.is_finite() || a < 0.0 => Err(AdaptError::NonFinite),
                    Some(a) => Ok(Some(a * self.transform.scale.abs())),
                    None => Ok(None),
                };
                let accuracy_m = metres(accuracy)?;
                let height_accuracy_m = metres(height_accuracy)?;
                let heading_deg = match heading {
                    Some(h) if !h.is_finite() => return Err(AdaptError::NonFinite),
                    Some(h) => Some(self.transform.apply_heading(h)),
                    None => None,
                };
                RadarEvent::Player(PlayerUpdate {
                    pos: self.pos(pos)?,
                    heading_deg,
                    zone: non_empty(zone),
                    observed_at: t,
                    accuracy_m,
                    height_accuracy_m,
                })
            }
            WireEvent::CubeSpawned {
                t,
                id,
                pos,
                label,
                zone,
                provenance,
            } => self.cube(
                CubeEventKind::Spawned,
                t,
                id,
                Some(pos),
                label,
                zone,
                provenance.into(),
            )?,
            WireEvent::CubeSeen {
                t,
                id,
                pos,
                zone,
                provenance,
            } => self.cube(
                CubeEventKind::Seen,
                t,
                id,
                pos,
                None,
                zone,
                provenance.into(),
            )?,
            WireEvent::CubeDespawned { t, id, zone } => self.cube(
                CubeEventKind::Despawned,
                t,
                id,
                None,
                None,
                zone,
                Provenance::Live,
            )?,
            WireEvent::CubeCollected { t, id, zone } => self.cube(
                CubeEventKind::Collected,
                t,
                id,
                None,
                None,
                zone,
                Provenance::Live,
            )?,
            WireEvent::CubeLost { t, id, zone } => self.cube(
                CubeEventKind::Lost,
                t,
                id,
                None,
                None,
                zone,
                Provenance::Live,
            )?,
            WireEvent::ZoneChanged { t, zone } => {
                if zone.trim().is_empty() {
                    return Err(AdaptError::EmptyZone);
                }
                RadarEvent::ZoneChanged {
                    zone,
                    observed_at: t,
                }
            }
            WireEvent::SpawnPoint {
                t,
                id,
                pos,
                label,
                zone,
            } => self.cube(
                CubeEventKind::Spawned,
                t.unwrap_or(0),
                id,
                Some(pos),
                label,
                zone,
                Provenance::StaticSpawnPoint,
            )?,
            WireEvent::HistoricalLocation {
                t,
                id,
                pos,
                label,
                zone,
            } => self.cube(
                CubeEventKind::Spawned,
                t,
                id,
                Some(pos),
                label,
                zone,
                Provenance::Historical,
            )?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn cube(
        &self,
        kind: CubeEventKind,
        t: Millis,
        id: Option<WireId>,
        pos: Option<[f64; 3]>,
        label: Option<String>,
        zone: Option<String>,
        provenance: Provenance,
    ) -> Result<RadarEvent, AdaptError> {
        Ok(RadarEvent::Cube(CubeEvent {
            id: id.map(WireId::into_string).and_then(|s| non_empty(Some(s))),
            kind,
            pos: pos.map(|p| self.pos(p)).transpose()?,
            label: non_empty(label),
            zone: non_empty(zone),
            provenance,
            observed_at: t,
        }))
    }

    fn pos(&self, p: [f64; 3]) -> Result<radar_core::WorldPos, AdaptError> {
        if !p.iter().all(|v| v.is_finite()) {
            return Err(AdaptError::NonFinite);
        }
        Ok(self.transform.apply(p))
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

impl From<WireProvenance> for Provenance {
    fn from(p: WireProvenance) -> Self {
        match p {
            WireProvenance::Live => Provenance::Live,
            WireProvenance::Historical => Provenance::Historical,
            WireProvenance::Predicted => Provenance::Predicted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use radar_core::{SourceAxis, WorldPos};

    fn identity() -> EventAdapter {
        EventAdapter::new(CoordinateTransform::default()).unwrap()
    }

    #[test]
    fn applies_the_coordinate_transform() {
        let adapter = EventAdapter::new(CoordinateTransform {
            north: SourceAxis::PosZ,
            up: SourceAxis::PosY,
            scale: 0.01,
            ..Default::default()
        })
        .unwrap();
        let ev = adapter
            .convert(WireEvent::CubeSpawned {
                t: 1,
                id: Some(WireId::Num(9)),
                pos: [100.0, 200.0, 300.0],
                label: Some("  ".into()),
                zone: None,
                provenance: WireProvenance::Live,
            })
            .unwrap();
        let RadarEvent::Cube(c) = ev else { panic!() };
        assert_eq!(c.id.as_deref(), Some("9"));
        assert_eq!(c.pos, Some(WorldPos::new(1.0, 3.0, 2.0)));
        assert_eq!(c.label, None);
        assert_eq!(c.provenance, Provenance::Live);
    }

    #[test]
    fn static_and_historical_records_are_tagged_not_dropped() {
        let a = identity();
        let RadarEvent::Cube(sp) = a
            .convert(WireEvent::SpawnPoint {
                t: None,
                id: Some("sp".into()),
                pos: [0.0; 3],
                label: None,
                zone: None,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(sp.provenance, Provenance::StaticSpawnPoint);

        let RadarEvent::Cube(h) = a
            .convert(WireEvent::HistoricalLocation {
                t: 3,
                id: Some("h".into()),
                pos: [0.0; 3],
                label: None,
                zone: None,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(h.provenance, Provenance::Historical);
    }

    #[test]
    fn heading_is_converted_and_non_finite_input_is_refused() {
        let a = EventAdapter::new(CoordinateTransform {
            heading_offset_deg: 90.0,
            ..Default::default()
        })
        .unwrap();
        let RadarEvent::Player(p) = a
            .convert(WireEvent::Player {
                accuracy: None,
                height_accuracy: None,
                t: 1,
                pos: [0.0; 3],
                heading: Some(300.0),
                zone: Some("".into()),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(p.heading_deg, Some(30.0));
        assert_eq!(p.zone, None);

        assert_eq!(
            a.convert(WireEvent::Player {
                accuracy: None,
                height_accuracy: None,
                t: 1,
                pos: [f64::NAN, 0.0, 0.0],
                heading: None,
                zone: None
            }),
            Err(AdaptError::NonFinite)
        );
        assert_eq!(
            a.convert(WireEvent::ZoneChanged {
                t: 1,
                zone: " ".into()
            }),
            Err(AdaptError::EmptyZone)
        );
    }

    #[test]
    fn invalid_transform_is_refused_at_construction() {
        let bad = CoordinateTransform {
            scale: 0.0,
            ..Default::default()
        };
        assert!(EventAdapter::new(bad).is_err());
    }
}
