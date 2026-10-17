//! Serves a SYNTHETIC stream over a real loopback TCP connection, in real
//! time, so live capture can be tested without the game:
//!
//!     cargo run -p radar-capture --example fake_server -- 7777 30          # invented opcodes
//!     cargo run -p radar-capture --example fake_server -- 7777 30 aion2    # documented AION 2 layouts
//!     cargo run -p radar-capture --example fake_server -- 7777 30 aion2-once   # …own position reported once
//!     cargo run -p radar-capture --example fake_server -- 7777 30 aion2-there  # …on another map (a teleport's target)
//!     cargo run -p radar-capture --example fake_server -- 7777 30 heartbeats   # only heartbeats (another connection)
//!     nc 127.0.0.1 7777 > /dev/null      # any client
//!
//! Either way the data is made up.

use std::io::Write;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use radar_capture::net::parse_ip;
use radar_capture::synthetic;

fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|a| a.parse().ok()).unwrap_or(7777);
    let seconds: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(30);
    use synthetic::aion2_layout::{heartbeat, session_on_map, session_with, Fixes};
    let records = match args.next().as_deref() {
        Some("aion2") => session_with(0.0, seconds, Fixes::Every3s),
        Some("aion2-once") => session_with(0.0, seconds, Fixes::Once),
        Some("aion2-there") => session_on_map(
            0.0,
            seconds,
            Fixes::Every3s,
            Some((2, [15_000.0, -10_000.0])),
        ),
        Some("heartbeats") => (0..seconds * 20)
            .map(|i| {
                let t = i as f64 * 50.0;
                let data = radar_capture::net::build_ipv4_tcp(
                    "198.51.100.40:9000".parse().unwrap(),
                    "192.168.0.10:50200".parse().unwrap(),
                    1_000,
                    0,
                    &heartbeat(t),
                );
                radar_capture::pcapfile::Record {
                    t_ms: t,
                    linktype: radar_capture::net::linktype::RAW,
                    data,
                }
            })
            .collect(),
        _ => synthetic::session(0.0, seconds).records,
    };
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    eprintln!("synthetic server on 127.0.0.1:{port}, waiting for a client");
    let (mut sock, peer) = listener.accept().expect("accept");
    eprintln!("client {peer}; streaming {seconds} s");
    let start = Instant::now();
    for r in &records {
        let Some(seg) = parse_ip(&r.data) else {
            continue;
        };
        if seg.payload.is_empty() {
            continue;
        }
        let due = start + Duration::from_millis(r.t_ms as u64);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        if sock.write_all(seg.payload).is_err() {
            break;
        }
    }
}
