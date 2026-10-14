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
