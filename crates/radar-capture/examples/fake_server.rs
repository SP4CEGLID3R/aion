//! Serves a SYNTHETIC stream over a real loopback TCP connection, in real
//! time, so live capture can be tested without the game:
//!
//!     cargo run -p radar-capture --example fake_server -- 7777 30          # invented opcodes
//!     cargo run -p radar-capture --example fake_server -- 7777 30 aion2    # documented AION 2 layouts
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
    let aion2 = args.next().as_deref() == Some("aion2");
    let records = if aion2 {
        synthetic::aion2_layout::session(0.0, seconds)
    } else {
        synthetic::session(0.0, seconds).records
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
