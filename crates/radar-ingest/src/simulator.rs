//! A deterministic synthetic world for development and tests.
//!
//! The player walks a looping path; cubes spawn near them, are re-confirmed
//! every second, and despawn when their lifetime ends or are collected when
//! the player walks over them. Some cubes are placed on the player's route
//! so collection actually happens.
//!
//! With `noise` on, the simulator also emits what a real source might mix in
//! and what must **never** become a marker: static spawn-point records,
//! historical locations of removed cubes, and duplicate spawn events. The
//! tests compare the radar's markers against [`Simulator::live_cubes`], the
//! simulator's ground truth.
//!
//! Everything it produces is made up. Nothing here reflects AION 2.

use std::f64::consts::TAU;

use radar_core::Millis;

use crate::wire::{WireEvent, WireId, WireProvenance};

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub seed: u64,
    pub cube_count: u32,
    pub zone: String,
    pub step_ms: Millis,
    pub seen_interval_ms: Millis,
    pub lifetime_ms: (Millis, Millis),
    pub spawn_radius: (f64, f64),
    pub collect_radius: f64,
    /// Emit spawn points, historical locations and duplicate spawns.
    pub noise: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            seed: 7,
            cube_count: 6,
            zone: "synthetic-zone".into(),
            step_ms: 50,
            seen_interval_ms: 1_000,
            lifetime_ms: (20_000, 60_000),
            spawn_radius: (30.0, 140.0),
            collect_radius: 4.0,
            noise: true,
        }
    }
}

/// SplitMix64: tiny, fast, and identical on every platform.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

#[derive(Debug, Clone)]
struct SimCube {
    id: String,
    pos: [f64; 3],
    expires_at: Millis,
}

#[derive(Debug, Clone)]
pub struct Simulator {
    cfg: SimConfig,
    rng: Rng,
    start: Millis,
    t: Millis,
    cubes: Vec<SimCube>,
    removed: Vec<(String, [f64; 3])>,
    next_id: u64,
    next_spawn_point: u64,
    last_seen_at: Millis,
    last_noise_at: Millis,
    started: bool,
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

impl Simulator {
    pub fn new(cfg: SimConfig, start: Millis) -> Self {
        let rng = Rng(cfg.seed);
        Self {
            cfg,
            rng,
            start,
            t: start,
            cubes: Vec::new(),
            removed: Vec::new(),
            next_id: 1,
            next_spawn_point: 1,
            last_seen_at: start,
            last_noise_at: start,
            started: false,
        }
    }

    pub fn config(&self) -> &SimConfig {
        &self.cfg
    }

    pub fn now(&self) -> Millis {
        self.t
    }

    /// Ground truth: the cubes that exist in the simulated world right now.
    pub fn live_cubes(&self) -> impl Iterator<Item = (&str, [f64; 3])> {
        self.cubes.iter().map(|c| (c.id.as_str(), c.pos))
    }

    /// Player position and compass heading at simulated time `t`.
    pub fn player_at(&self, t: Millis) -> ([f64; 3], f64) {
        let s = (t - self.start) as f64 / 1000.0;
        let (w1, w2) = (TAU / 90.0, TAU / 60.0);
        let x = 110.0 * (w1 * s).sin();
        let y = 80.0 * (w2 * s + 0.7).sin();
        let dx = 110.0 * w1 * (w1 * s).cos();
        let dy = 80.0 * w2 * (w2 * s + 0.7).cos();
        let heading = radar_core::geometry::normalize_deg(dx.atan2(dy).to_degrees());
        ([x, y, 0.0], heading)
    }

    /// Advances one step and returns what a source would report.
    pub fn step(&mut self) -> Vec<WireEvent> {
        let mut out = Vec::new();
        if self.started {
            self.t += self.cfg.step_ms;
        } else {
            self.started = true;
            out.push(WireEvent::ZoneChanged {
                t: self.t,
                zone: self.cfg.zone.clone(),
            });
        }
        let t = self.t;
        let (player, heading) = self.player_at(t);
        out.push(WireEvent::Player {
            accuracy: None,
            t,
            pos: player.map(round1),
            heading: Some(round1(heading)),
            zone: Some(self.cfg.zone.clone()),
        });

        // Removals first, so a cube is never reported after it is gone.
        let mut i = 0;
        while i < self.cubes.len() {
            let c = &self.cubes[i];
            let d = (c.pos[0] - player[0]).hypot(c.pos[1] - player[1]);
            let collected = d <= self.cfg.collect_radius;
            if collected || t >= c.expires_at {
                let c = self.cubes.swap_remove(i);
                let id = Some(WireId::Str(c.id.clone()));
                out.push(if collected {
                    WireEvent::CubeCollected { t, id, zone: None }
                } else {
                    WireEvent::CubeDespawned { t, id, zone: None }
                });
                self.removed.push((c.id, c.pos));
            } else {
                i += 1;
            }
        }

        while (self.cubes.len() as u32) < self.cfg.cube_count {
            let cube = self.new_cube(t, player);
            let spawn = WireEvent::CubeSpawned {
                t,
                id: Some(WireId::Str(cube.id.clone())),
                pos: cube.pos,
                label: Some(format!("Cube #{}", &cube.id[4..])),
                zone: Some(self.cfg.zone.clone()),
                provenance: WireProvenance::Live,
            };
            if self.cfg.noise && self.rng.unit() < 0.3 {
                out.push(spawn.clone()); // a duplicate, as a real source may send
            }
            out.push(spawn);
            self.cubes.push(cube);
        }

        if t - self.last_seen_at >= self.cfg.seen_interval_ms {
            self.last_seen_at = t;
            for c in &self.cubes {
                out.push(WireEvent::CubeSeen {
                    t,
                    id: Some(WireId::Str(c.id.clone())),
                    pos: None,
                    zone: None,
                    provenance: WireProvenance::Live,
                });
            }
        }

        if self.cfg.noise && t - self.last_noise_at >= 5_000 {
            self.last_noise_at = t;
            let n = self.next_spawn_point;
            self.next_spawn_point += 1;
            let a = self.rng.range(0.0, TAU);
            let r = self.rng.range(10.0, 120.0);
            out.push(WireEvent::SpawnPoint {
                t: None,
                id: Some(WireId::Str(format!("spawn-point-{n}"))),
                pos: [
                    round1(player[0] + r * a.sin()),
                    round1(player[1] + r * a.cos()),
                    0.0,
                ],
                label: Some("Possible cube spawn".into()),
                zone: Some(self.cfg.zone.clone()),
            });
            if let Some((id, pos)) = self.removed.last() {
                out.push(WireEvent::HistoricalLocation {
                    t: t - 1,
                    id: Some(WireId::Str(id.clone())),
                    pos: *pos,
                    label: Some("Last known location".into()),
                    zone: Some(self.cfg.zone.clone()),
                });
            }
        }
        out
    }

    fn new_cube(&mut self, t: Millis, player: [f64; 3]) -> SimCube {
        let id = format!("sim-{}", self.next_id);
        self.next_id += 1;
        let lifetime = self
            .rng
            .range(self.cfg.lifetime_ms.0 as f64, self.cfg.lifetime_ms.1 as f64);
        let pos = if self.rng.unit() < 0.3 {
            // On the route a few seconds ahead, so the player collects it.
            let ahead = t + self.rng.range(4_000.0, 12_000.0) as Millis;
            self.player_at(ahead).0
        } else {
            let a = self.rng.range(0.0, TAU);
            let r = self
                .rng
                .range(self.cfg.spawn_radius.0, self.cfg.spawn_radius.1);
            [
                player[0] + r * a.sin(),
                player[1] + r * a.cos(),
                self.rng.range(-3.0, 8.0),
            ]
        };
        SimCube {
            id,
            pos: pos.map(round1),
            expires_at: t + lifetime as Millis,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic_for_a_seed() {
        let run = |seed| {
            let mut s = Simulator::new(
                SimConfig {
                    seed,
                    ..Default::default()
                },
                0,
            );
            (0..400).flat_map(|_| s.step()).collect::<Vec<_>>()
        };
        assert_eq!(run(1), run(1));
        assert_ne!(run(1), run(2));
    }

    #[test]
    fn keeps_the_cube_count_and_eventually_collects_one() {
        let mut s = Simulator::new(SimConfig::default(), 0);
        let mut collected = 0;
        let mut despawned = 0;
        for _ in 0..(120_000 / 50) {
            for ev in s.step() {
                match ev {
                    WireEvent::CubeCollected { .. } => collected += 1,
                    WireEvent::CubeDespawned { .. } => despawned += 1,
                    _ => {}
                }
            }
            assert_eq!(s.live_cubes().count(), 6);
        }
        assert!(collected > 0, "nothing was collected in two minutes");
        assert!(despawned > 0);
    }

    #[test]
    fn heading_points_along_the_path() {
        let s = Simulator::new(SimConfig::default(), 0);
        let (a, h) = s.player_at(10_000);
        let (b, _) = s.player_at(10_050);
        let travel = (b[0] - a[0]).atan2(b[1] - a[1]).to_degrees();
        let diff = (radar_core::geometry::normalize_deg(travel) - h).abs();
        assert!(
            !(1.0..=359.0).contains(&diff),
            "heading {h} vs travel {travel}"
        );
    }
}
