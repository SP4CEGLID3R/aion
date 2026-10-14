//! Data sources. Each runs on its own thread and pushes protocol events out
//! through a callback; it never touches tracker state directly.

use std::fs;
use std::io::{ErrorKind, Read};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use radar_core::{DataSourceConfig, Millis, SourceState};
use thiserror::Error;

use crate::clock::now_ms;
use crate::simulator::{SimConfig, Simulator};
use crate::srf;
use crate::wire::{self, WireEvent, MAX_LINE_BYTES};

/// Largest replay file accepted.
pub const MAX_REPLAY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum SourceMessage {
    Event(WireEvent),
    Status(SourceState, String),
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
}

/// Callback a source emits through. Returns `false` once nobody is listening,
/// and the source should then return.
pub type Emit<'a> = dyn FnMut(SourceMessage) -> bool + 'a;

pub trait DataSource: Send {
    fn name(&self) -> String;
    /// Runs until `stop` is set, the consumer goes away, or the source ends.
    fn run(&mut self, emit: &mut Emit<'_>, stop: &AtomicBool) -> Result<(), SourceError>;
}

/// Builds the source a configuration asks for; `None` for
/// [`DataSourceConfig::None`].
pub fn from_config(config: &DataSourceConfig) -> Option<Box<dyn DataSource>> {
    match config {
        DataSourceConfig::Synthetic { seed, cube_count } => {
            Some(Box::new(SimulatorSource::new(SimConfig {
                seed: *seed,
                cube_count: *cube_count,
                ..Default::default()
            })))
        }
        DataSourceConfig::Replay {
            path,
            speed,
            looped,
        } => Some(Box::new(ReplaySource::new(
            PathBuf::from(path),
            *speed,
            *looped,
        ))),
        DataSourceConfig::LocalTcp { port } => Some(Box::new(LocalTcpSource::new(*port))),
        DataSourceConfig::None => None,
    }
}

/// Sleeps up to `d`, waking early when `stop` is set. Returns `false` if stopped.
fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) -> bool {
    let end = Instant::now() + d;
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = Instant::now();
        if now >= end {
            return true;
        }
        thread::sleep((end - now).min(Duration::from_millis(50)));
    }
}

// ---------------------------------------------------------------------------
// Synthetic simulator
// ---------------------------------------------------------------------------

pub struct SimulatorSource {
    cfg: SimConfig,
}

impl SimulatorSource {
    pub fn new(cfg: SimConfig) -> Self {
        Self { cfg }
    }
}

impl DataSource for SimulatorSource {
    fn name(&self) -> String {
        format!("Synthetic simulator (seed {})", self.cfg.seed)
    }

    fn run(&mut self, emit: &mut Emit<'_>, stop: &AtomicBool) -> Result<(), SourceError> {
        let step = Duration::from_millis(self.cfg.step_ms as u64);
        let mut sim = Simulator::new(self.cfg.clone(), now_ms());
        if !emit(SourceMessage::Status(
            SourceState::Connected,
            "synthetic data — not from the game".into(),
        )) {
            return Ok(());
        }
        let started = Instant::now();
        let mut steps: u32 = 0;
        loop {
            for ev in sim.step() {
                if !emit(SourceMessage::Event(ev)) {
                    return Ok(());
                }
            }
            steps += 1;
            // Pace against the start so timing does not drift.
            let due = started + step * steps;
            if !sleep_unless_stopped(due.saturating_duration_since(Instant::now()), stop) {
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// File replay
// ---------------------------------------------------------------------------

pub struct ReplaySource {
    path: PathBuf,
    speed: f64,
    looped: bool,
}

/// Loaded replay content, plus how many lines/frames could not be read.
#[derive(Debug, Default)]
pub struct LoadedReplay {
    pub events: Vec<WireEvent>,
    pub errors: Vec<String>,
}

/// Reads a `.jsonl`, `.srf` or `.srf.hex` file into protocol events.
pub fn load_replay(path: &Path) -> Result<LoadedReplay, SourceError> {
    let size = fs::metadata(path)?.len();
    if size > MAX_REPLAY_BYTES {
        return Err(SourceError::Invalid(format!(
            "{} is {size} bytes; the limit is {MAX_REPLAY_BYTES}",
            path.display()
        )));
    }
    let name = path.to_string_lossy().to_lowercase();
    let mut out = LoadedReplay::default();
    if name.ends_with(".srf") || name.ends_with(".srf.hex") {
        let bytes = if name.ends_with(".hex") {
            srf::parse_hex_fixture(&fs::read_to_string(path)?).map_err(SourceError::Invalid)?
        } else {
            fs::read(path)?
        };
        let (frames, stats) = srf::decode_all(&bytes);
        for (i, f) in frames.into_iter().enumerate() {
            match f {
                Ok(ev) => out.events.push(ev),
                Err(e) => out.errors.push(format!("frame {}: {e}", i + 1)),
            }
        }
        if stats.unknown_opcodes > 0 {
            out.errors.push(format!(
                "{} frames with unknown opcodes skipped",
                stats.unknown_opcodes
            ));
        }
    } else {
        for (i, line) in fs::read_to_string(path)?.lines().enumerate() {
            match wire::parse_line(line) {
                Ok(Some(ev)) => out.events.push(ev),
                Ok(None) => {}
                Err(e) => out.errors.push(format!("line {}: {e}", i + 1)),
            }
        }
    }
    Ok(out)
}

impl ReplaySource {
    pub fn new(path: PathBuf, speed: f64, looped: bool) -> Self {
        let speed = if speed.is_finite() && speed > 0.0 {
            speed
        } else {
            1.0
        };
        Self {
            path,
            speed,
            looped,
        }
    }
}

impl DataSource for ReplaySource {
    fn name(&self) -> String {
        format!("Replay {}", self.path.display())
    }

    fn run(&mut self, emit: &mut Emit<'_>, stop: &AtomicBool) -> Result<(), SourceError> {
        let loaded = load_replay(&self.path)?;
        for e in loaded.errors.iter().take(20) {
            log::warn!("replay {}: {e}", self.path.display());
        }
        if loaded.events.is_empty() {
            return Err(SourceError::Invalid(format!(
                "{} holds no readable events",
                self.path.display()
            )));
        }
        let detail = if loaded.errors.is_empty() {
            format!("{} events", loaded.events.len())
        } else {
            format!(
                "{} events, {} unreadable",
                loaded.events.len(),
                loaded.errors.len()
            )
        };
        if !emit(SourceMessage::Status(SourceState::Connected, detail)) {
            return Ok(());
        }
        loop {
            // Timestamps are rebased so the recording plays "now": an event
            // recorded at t is stamped start + (t - t0) / speed, which is
            // also when it is emitted.
            let t0 = loaded.events.iter().find_map(WireEvent::time).unwrap_or(0);
            let start_ms = now_ms();
            let start = Instant::now();
            for ev in &loaded.events {
                let mut ev = ev.clone();
                if let Some(t) = ev.time() {
                    let offset = ((t - t0).max(0) as f64 / self.speed) as Millis;
                    let due = start + Duration::from_millis(offset as u64);
                    if !sleep_unless_stopped(due.saturating_duration_since(Instant::now()), stop) {
                        return Ok(());
                    }
                    ev.shift_time(start_ms + offset - t);
                }
                if !emit(SourceMessage::Event(ev)) {
                    return Ok(());
                }
            }
            if !self.looped {
                emit(SourceMessage::Status(
                    SourceState::Finished,
                    "replay finished".into(),
                ));
                return Ok(());
            }
            if !sleep_unless_stopped(Duration::from_millis(500), stop) {
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Local TCP (loopback only)
// ---------------------------------------------------------------------------

/// Splits a byte stream into lines, enforcing [`MAX_LINE_BYTES`].
#[derive(Debug, Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
    discarding: bool,
}

#[derive(Debug, PartialEq)]
pub enum LineError {
    TooLong,
    NotUtf8,
}

impl LineSplitter {
    pub fn push(&mut self, mut bytes: &[u8]) -> Vec<Result<String, LineError>> {
        let mut out = Vec::new();
        while let Some(nl) = bytes.iter().position(|&b| b == b'\n') {
            let (head, rest) = bytes.split_at(nl);
            bytes = &rest[1..];
            if self.discarding {
                self.discarding = false;
                self.pending.clear();
                continue;
            }
            self.pending.extend_from_slice(head);
            let line = std::mem::take(&mut self.pending);
            if line.len() > MAX_LINE_BYTES {
                out.push(Err(LineError::TooLong));
                continue;
            }
            out.push(String::from_utf8(line).map_err(|_| LineError::NotUtf8));
        }
        if !self.discarding {
            self.pending.extend_from_slice(bytes);
            if self.pending.len() > MAX_LINE_BYTES {
                self.pending.clear();
                self.discarding = true;
                out.push(Err(LineError::TooLong));
            }
        }
        out
    }
}

pub struct LocalTcpSource {
    port: u16,
    bound: Option<SocketAddr>,
}

impl LocalTcpSource {
    pub fn new(port: u16) -> Self {
        Self { port, bound: None }
    }

    fn serve(&self, mut stream: TcpStream, emit: &mut Emit<'_>, stop: &AtomicBool) -> bool {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let mut splitter = LineSplitter::default();
        let mut buf = [0u8; 8192];
        let mut bad_lines: u64 = 0;
        loop {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            let n = match stream.read(&mut buf) {
                Ok(0) => return true,
                Ok(n) => n,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    continue
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    log::warn!("telemetry client read failed: {e}");
                    return true;
                }
            };
            for line in splitter.push(&buf[..n]) {
                let parsed = match line {
                    Ok(l) => wire::parse_line(&l).map_err(|e| e.to_string()),
                    Err(e) => Err(format!("{e:?}")),
                };
                match parsed {
                    Ok(Some(ev)) => {
                        if !emit(SourceMessage::Event(ev)) {
                            return false;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        bad_lines += 1;
                        if bad_lines <= 10 || bad_lines % 100 == 0 {
                            log::warn!("telemetry line rejected ({bad_lines} so far): {e}");
                        }
                    }
                }
            }
        }
    }
}

impl DataSource for LocalTcpSource {
    fn name(&self) -> String {
        format!("Local telemetry 127.0.0.1:{}", self.port)
    }

    fn run(&mut self, emit: &mut Emit<'_>, stop: &AtomicBool) -> Result<(), SourceError> {
        // Loopback only: this socket is never reachable from the network.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.port))?;
        listener.set_nonblocking(true)?;
        self.bound = Some(listener.local_addr()?);
        let addr = self.bound.expect("just set");
        if !emit(SourceMessage::Status(
            SourceState::Waiting,
            format!("listening on {addr}"),
        )) {
            return Ok(());
        }
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            match listener.accept() {
                Ok((stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue; // cannot happen on a loopback bind; refuse anyway
                    }
                    if !emit(SourceMessage::Status(
                        SourceState::Connected,
                        format!("client {peer}"),
                    )) {
                        return Ok(());
                    }
                    if !self.serve(stream, emit, stop) {
                        return Ok(());
                    }
                    if !emit(SourceMessage::Status(
                        SourceState::Waiting,
                        format!("client left; listening on {addr}"),
                    )) {
                        return Ok(());
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if !sleep_unless_stopped(Duration::from_millis(100), stop) {
                        return Ok(());
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;
    use std::sync::Arc;

    #[test]
    fn line_splitter_handles_chunks_and_limits() {
        let mut s = LineSplitter::default();
        assert!(s.push(b"ab").is_empty());
        assert_eq!(s.push(b"c\nde\n"), vec![Ok("abc".into()), Ok("de".into())]);
        assert_eq!(s.push(&[0xff, b'\n']), vec![Err(LineError::NotUtf8)]);

        let big = vec![b'x'; MAX_LINE_BYTES + 1];
        assert_eq!(s.push(&big), vec![Err(LineError::TooLong)]);
        // The rest of the oversized line is dropped, the next one is kept.
        assert_eq!(s.push(b"yyy\nok\n"), vec![Ok("ok".into())]);
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn loads_jsonl_and_srf_fixtures() {
        let jsonl = load_replay(&fixture("acceptance.jsonl")).unwrap();
        assert!(jsonl.errors.is_empty(), "{:?}", jsonl.errors);
        assert!(jsonl.events.len() > 10);

        let srf = load_replay(&fixture("synthetic-session.srf.hex")).unwrap();
        assert!(srf.errors.is_empty(), "{:?}", srf.errors);
        assert!(!srf.events.is_empty());

        let bad = load_replay(&fixture("malformed.jsonl")).unwrap();
        assert_eq!(bad.events.len(), 2);
        assert_eq!(bad.errors.len(), 4, "{:?}", bad.errors);
    }

    #[test]
    fn replay_rebases_timestamps_to_now() {
        let mut src = ReplaySource::new(fixture("acceptance.jsonl"), 100.0, false);
        let stop = AtomicBool::new(false);
        let mut got = Vec::new();
        let before = now_ms();
        src.run(
            &mut |m| {
                got.push(m);
                true
            },
            &stop,
        )
        .unwrap();
        let after = now_ms();
        assert!(matches!(
            got.last(),
            Some(SourceMessage::Status(SourceState::Finished, _))
        ));
        for m in &got {
            if let SourceMessage::Event(ev) = m {
                if let Some(t) = ev.time() {
                    assert!(
                        t >= before - 1 && t <= after + 1,
                        "{t} not in [{before}, {after}]"
                    );
                }
            }
        }
    }

    #[test]
    fn replay_of_a_missing_file_is_an_error() {
        let mut src = ReplaySource::new(fixture("does-not-exist.jsonl"), 1.0, false);
        let stop = AtomicBool::new(false);
        assert!(src.run(&mut |_| true, &stop).is_err());
    }

    #[test]
    fn local_tcp_receives_lines_from_a_loopback_client() {
        // Port 0: let the OS pick, then read it back from the status message.
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stop2 = stop.clone();
        let handle = thread::spawn(move || {
            let mut src = LocalTcpSource::new(0);
            src.run(&mut |m| tx.send(m).is_ok(), &stop2)
        });
        let SourceMessage::Status(SourceState::Waiting, detail) = rx.recv().unwrap() else {
            panic!("expected a listening status")
        };
        let addr: SocketAddr = detail.trim_start_matches("listening on ").parse().unwrap();
        assert!(addr.ip().is_loopback());

        let mut client = TcpStream::connect(addr).unwrap();
        client
            .write_all(b"{\"v\":1,\"type\":\"zone_changed\",\"t\":1,\"zone\":\"z\"}\nnot json\n")
            .unwrap();
        client
            .write_all(b"{\"v\":1,\"type\":\"cube_seen\",\"t\":2,\"id\":\"a\"}\n")
            .unwrap();
        drop(client);

        let mut events = Vec::new();
        while events.len() < 2 {
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                SourceMessage::Event(e) => events.push(e),
                SourceMessage::Status(..) => {}
            }
        }
        assert!(matches!(events[0], WireEvent::ZoneChanged { .. }));
        assert!(matches!(events[1], WireEvent::CubeSeen { .. }));
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
    }
}
