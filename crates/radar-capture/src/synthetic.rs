//! A synthetic capture for trying the discovery workflow end to end.
//!
//! **Everything here is invented.** The framing follows the documented AION 2
//! rules (so the real framer is exercised), but the opcodes `C1 7E` (cube
//! spawn/collect) and `C2 7E` (player position) and their layouts are made up
//! for this demo. They say nothing about the real game.

use std::net::SocketAddr;

use crate::discovery::Marker;
use crate::framing::{encode_bundle, encode_frame};
use crate::net::{build_ipv4_tcp, linktype, tcp_flags};
use crate::pcapfile::Record;

pub const SERVER: &str = "198.51.100.7:7777";
pub const CLIENT: &str = "192.168.0.10:55555";

/// The profile that decodes this synthetic session (and nothing real).
pub const PROFILE_JSON: &str = r#"{
  "name": "SYNTHETIC DEMO — invented opcodes, not AION 2",
  "gameVersion": "none (synthetic)",
  "validated": true,
  "notes": "Decodes the output of `radar-capture synth` only.",
  "messages": [
    { "opcode": "C1 7E", "event": "cube_spawned", "when": [{ "offset": 2, "equals": "01" }],
      "label": "Cube",
      "fields": { "id": { "offset": 3, "type": "varint" },
                  "x": { "after": "id", "offset": 0, "type": "f32le" },
                  "y": { "after": "id", "offset": 4, "type": "f32le" },
                  "z": { "after": "id", "offset": 8, "type": "f32le" } } },
    { "opcode": "C1 7E", "event": "cube_seen", "when": [{ "offset": 2, "equals": "03" }],
      "fields": { "id": { "offset": 3, "type": "varint" } } },
    { "opcode": "C1 7E", "event": "cube_collected", "when": [{ "offset": 2, "equals": "02" }],
      "fields": { "id": { "offset": 3, "type": "varint" } } },
    { "opcode": "C2 7E", "event": "player",
      "fields": { "x": { "offset": 2, "type": "f32le" },
                  "y": { "offset": 6, "type": "f32le" },
                  "z": { "offset": 10, "type": "f32le" },
                  "heading": { "offset": 14, "type": "u16le", "scale": 0.0054931640625 } } }
  ]
}"#;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

fn varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

pub struct Session {
    pub records: Vec<Record>,
    pub markers: Vec<Marker>,
    /// (id, spawn ms, collect ms, position) of every cube, for checking.
    pub cubes: Vec<(u64, f64, f64, [f32; 3])>,
}

/// `seconds` of synthetic game traffic starting at `t0` (ms).
pub fn session(t0: f64, seconds: u32) -> Session {
    let server: SocketAddr = SERVER.parse().unwrap();
    let client: SocketAddr = CLIENT.parse().unwrap();
    let mut rng = Lcg(42);
    let mut seq: u32 = 1_000;
    let mut records = vec![Record {
        t_ms: t0,
        linktype: linktype::RAW,
        data: build_ipv4_tcp(server, client, seq - 1, tcp_flags::SYN, &[]),
    }];
    let mut send = |t: f64, bytes: Vec<u8>, records: &mut Vec<Record>| {
        for chunk in bytes.chunks(1400) {
            records.push(Record {
                t_ms: t,
                linktype: linktype::RAW,
                data: build_ipv4_tcp(server, client, seq, 0, chunk),
            });
            seq = seq.wrapping_add(chunk.len() as u32);
        }
    };

    let mut cubes = Vec::new();
    let mut markers = Vec::new();
    let mut next_id = 9_000u64;
    let player = |t: f64| {
        let s = (t - t0) / 1000.0;
        [
            (s * 0.3).sin() as f32 * 60.0,
            (s * 0.2).cos() as f32 * 40.0,
            2.0f32,
        ]
    };

    let step = 50.0;
    let mut t = t0;
    let end = t0 + seconds as f64 * 1000.0;
    let mut live: Vec<(u64, f64, [f32; 3])> = Vec::new();
    let mut tick: u64 = 0;
    while t < end {
        let mut batch = encode_frame([0x00, 0x36], &[0u8; 8]); // heartbeat
        if tick.is_multiple_of(2) {
            let p = player(t);
            let mut body = Vec::new();
            for v in p {
                body.extend(v.to_le_bytes());
            }
            body.extend(((tick * 300) as u16).to_le_bytes());
            batch.extend(encode_frame([0xC2, 0x7E], &body));
        }
        if tick.is_multiple_of(4) {
            // Background noise: some other packet type, sometimes bundled.
            let noise: Vec<u8> = (0..(20 + rng.next() % 60))
                .map(|_| rng.next() as u8)
                .collect();
            let f = encode_frame([0x04, 0x38], &noise);
            batch.extend(if rng.next().is_multiple_of(3) {
                encode_bundle(&f)
            } else {
                f
            });
        }
        // A cube spawns every ~15 s, a little ahead of the player.
        if tick % 300 == 150 {
            let id = next_id;
            next_id += 1;
            let p = player(t);
            let pos = [p[0] + 25.0, p[1] - 15.0, 1.0];
            let mut body = vec![0x01];
            varint(id, &mut body);
            for v in pos {
                body.extend(v.to_le_bytes());
            }
            batch.extend(encode_bundle(&encode_frame([0xC1, 0x7E], &body)));
            live.push((id, t, pos));
            markers.push(Marker {
                t: t + 350.0, // the person presses the key a moment after seeing it
                kind: "cube_visible".into(),
                note: None,
            });
        }
        if tick.is_multiple_of(20) {
            for (id, _, _) in &live {
                let mut body = vec![0x03];
                varint(*id, &mut body);
                batch.extend(encode_frame([0xC1, 0x7E], &body));
            }
        }
        // Collected 8 s after spawning.
        live.retain(|(id, spawned, pos)| {
            if t - spawned >= 8_000.0 {
                let mut body = vec![0x02];
                varint(*id, &mut body);
                batch.extend(encode_frame([0xC1, 0x7E], &body));
                cubes.push((*id, *spawned, t, *pos));
                markers.push(Marker {
                    t: t - 200.0,
                    kind: "cube_collected".into(),
                    note: None,
                });
                false
            } else {
                true
            }
        });
        send(t, batch, &mut records);
        t += step;
        tick += 1;
    }
    for (id, spawned, pos) in live {
        cubes.push((id, spawned, f64::INFINITY, pos));
    }
    Session {
        records,
        markers,
        cubes,
    }
}

/// A made-up session in the **documented AION 2 layouts** (see `aion2.rs`),
/// for demos and tests of the built-in decoder. Positions, ids and timing are
/// invented; no captured game data is used.
pub mod aion2_layout {
    use std::net::SocketAddr;

    use crate::framing::{encode_bundle, encode_frame};
    use crate::net::{build_ipv4_tcp, linktype, tcp_flags};
    use crate::pcapfile::Record;

    pub const SELF_ID: u64 = 9_180;
    pub const CUBE_TEMPLATE: u32 = 1_200_039;

    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return out;
            }
            out.push(b | 0x80);
        }
    }

    fn xyz(p: [f32; 3]) -> Vec<u8> {
        p.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    pub fn heartbeat(t: f64) -> Vec<u8> {
        encode_frame([0x00, 0x36], &(t as u64).to_le_bytes())
    }

    pub fn spawn(id: u64, template: u32, p: [f32; 3]) -> Vec<u8> {
        let mut b = varint(id);
        b.extend([0x04, 0x00]);
        b.extend(template.to_le_bytes());
        b.extend(xyz(p));
        b.extend([0u8; 8]);
        b.extend([1, 1, 1]);
        b.extend(1_791_884_700_000i64.to_le_bytes());
        encode_frame([0x34, 0x36], &b)
    }

    pub fn removed(id: u64, reason: u8) -> Vec<u8> {
        let mut b = varint(id);
        b.extend([reason, 0x03, 0x00]);
        encode_frame([0x35, 0x36], &b)
    }

    /// Map load: the map id and our own position (after a teleport too).
    pub fn map_load(map: u32, p: [f32; 3]) -> Vec<u8> {
        let mut b = 11u32.to_le_bytes().to_vec();
        b.extend(map.to_le_bytes());
        b.extend([0x3a, 0x03, 0x87, 0x00, 0, 0, 0, 0]);
        b.extend(xyz(p));
        b.extend(180f32.to_le_bytes());
        b.extend([2, 0, 0, 0]);
        encode_frame([0x21, 0x36], &b)
    }

    /// Who we are (sent once at login).
    pub fn self_info() -> Vec<u8> {
        let mut b = varint(SELF_ID);
        b.extend([0u8; 8]);
        encode_frame([0x33, 0x36], &b)
    }

    pub fn self_buff(p: [f32; 3]) -> Vec<u8> {
        let mut b = varint(SELF_ID);
        b.extend([0x01, 0x11, 0x90, 0x03, 0xe8, 0x03, 0, 0, 0, 0, 0, 0, 0, 0]);
        b.extend(1_791_884_800_000i64.to_le_bytes());
        b.extend(varint(SELF_ID));
        b.extend([0x01, 0x00]);
        b.extend(xyz(p));
        encode_frame([0x2a, 0x38], &b)
    }

    /// Player position (game units, cm) at `rel` ms: a slow walk.
    pub fn player(rel: f64) -> [f32; 3] {
        let s = rel / 1000.0;
        [
            -45_000.0 + (s * 0.05).sin() as f32 * 3_000.0,
            120_000.0 + (s * 0.04).cos() as f32 * 3_000.0,
            15_000.0,
        ]
    }

    /// How often the synthetic session reports our own position.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Fixes {
        /// A self-buff every 3 s.
        Every3s,
        /// Who we are, then one self-buff after 1 s and never again (like
        /// opening the wings once after logging in).
        Once,
    }

    /// `seconds` of a server→client game stream starting at `t0` (ms): a
    /// heartbeat every 50 ms, a self-buff every 3 s, a cube appearing about
    /// 60 m away (12 m up, 5 m down or level) every 20 s; each is opened
    /// after 12 s (or, every third one, leaves the send range), plus objects
    /// that are not cubes, some standing around on flat ground.
    pub fn session(t0: f64, seconds: u32) -> Vec<Record> {
        session_with(t0, seconds, Fixes::Every3s)
    }

    pub fn session_with(t0: f64, seconds: u32, fixes: Fixes) -> Vec<Record> {
        session_on_map(t0, seconds, fixes, None)
    }

    /// The same, on another map: `Some((map id, shift))` starts with a map
    /// load (as after a teleport) and everything is `shift` (game units)
    /// away from the usual walk.
    pub fn session_on_map(
        t0: f64,
        seconds: u32,
        fixes: Fixes,
        map: Option<(u32, [f32; 2])>,
    ) -> Vec<Record> {
        let shift = map.map_or([0.0, 0.0], |(_, s)| s);
        let player = |rel: f64| {
            let p = player(rel);
            [p[0] + shift[0], p[1] + shift[1], p[2]]
        };
        let server: SocketAddr = "198.51.100.30:13328".parse().unwrap();
        let client: SocketAddr = "192.168.0.10:50123".parse().unwrap();
        let mut seq: u32 = 1_000;
        let mut records = vec![Record {
            t_ms: t0,
            linktype: linktype::RAW,
            data: build_ipv4_tcp(server, client, seq - 1, tcp_flags::SYN, &[]),
        }];
        let mut live: Vec<(u64, f64, bool)> = Vec::new();
        let mut next_id = 65_000u64;
        let mut tick: u64 = 0;
        while (tick as f64) * 50.0 < seconds as f64 * 1000.0 {
            let rel = tick as f64 * 50.0;
            let t = t0 + rel;
            let mut batch = heartbeat(t);
            if tick == 0 {
                if let Some((id, _)) = map {
                    batch.extend(map_load(id, player(0.0)));
                }
                if fixes == Fixes::Once {
                    batch.extend(self_info());
                }
                // Things standing around on the (flat) ground near the walk.
                for k in 0..36u64 {
                    let a = k as f32 * 0.7;
                    let r = 1_000.0 + (k % 6) as f32 * 700.0;
                    let c = player(0.0);
                    let wobble = (k % 5) as f32 * 10.0 - 20.0;
                    batch.extend(spawn(
                        80_000 + k,
                        1_500_050,
                        [c[0] + r * a.cos(), c[1] + r * a.sin(), c[2] + wobble],
                    ));
                }
            }
            let fix_due = match fixes {
                Fixes::Every3s => tick.is_multiple_of(60),
                Fixes::Once => tick == 20,
            };
            if fix_due {
                batch.extend(self_buff(player(rel)));
            }
            if tick % 400 == 40 {
                let p = player(rel);
                let a = tick as f32 * 0.37;
                let id = next_id;
                // Some up on a ledge, some down in a hollow, some level.
                let up = [1_200.0, -500.0, 100.0][(id % 3) as usize];
                let pos = [
                    p[0] + 6_000.0 * a.cos(),
                    p[1] + 6_000.0 * a.sin(),
                    p[2] + up,
                ];
                next_id += 1;
                let frame = spawn(id, CUBE_TEMPLATE, pos);
                // Some arrive inside compressed bundles, like real traffic.
                batch.extend(if id.is_multiple_of(2) {
                    encode_bundle(&frame)
                } else {
                    frame
                });
                batch.extend(spawn(
                    next_id + 10_000,
                    1_500_041,
                    [pos[0] + 900.0, pos[1], pos[2]],
                ));
                live.push((id, rel, id.is_multiple_of(3)));
            }
            live.retain(|(id, born, leaves)| {
                if rel - born >= 12_000.0 {
                    batch.extend(removed(*id, if *leaves { 0x01 } else { 0x04 }));
                    false
                } else {
                    true
                }
            });
            for chunk in batch.chunks(1400) {
                records.push(Record {
                    t_ms: t,
                    linktype: linktype::RAW,
                    data: build_ipv4_tcp(server, client, seq, 0, chunk),
                });
                seq = seq.wrapping_add(chunk.len() as u32);
            }
            tick += 1;
        }
        records
    }
}
