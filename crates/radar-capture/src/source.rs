//! The capture data source: packets → TCP → AION 2 frames → census, optional
//! recording, and radar events from the built-in AION 2 decoder or a
//! validated decoder profile.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use radar_core::{Millis, SourceState};
use radar_ingest::clock::now_ms;
use radar_ingest::source::{DataSource, Emit, SourceError};
use radar_ingest::{SourceMessage, WireEvent};
use serde::Serialize;

use crate::aion2::{Aion2Config, Aion2Decoder, Aion2Status};
use crate::census::{Census, OpcodeRow};
use crate::discovery::{append_marker, Marker};
use crate::follow::MapConnection;
use crate::live::{Next, PcapApi};
use crate::minimap::MinimapLink;
use crate::net::{self, linktype};
use crate::pcapfile::{self, PcapWriter};
use crate::profile::{Decoder, DecoderProfile};
use crate::streams::{FlowKind, GameStreams, StreamEvent, StreamStats};

const MAX_RECORDING_BYTES: u64 = 1024 * 1024 * 1024;
/// Raw packets of a not-yet-recognised flow kept for the recording.
const MAX_PENDING_PACKETS: usize = 2_000;
const STATUS_EVERY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub enum CaptureInput {
    /// Live capture. `None` = every adapter (the game is found by content).
    Live { device: Option<String> },
    /// A `.pcap`/`.pcapng` file, played back in real time.
    File { path: PathBuf },
}

/// What turns game frames into radar events.
#[derive(Debug, Clone)]
pub enum DecoderChoice {
    /// The built-in hidden-cube decoder (layouts documented in `aion2.rs`).
    Aion2(Box<Aion2Config>),
    /// A user-written profile; draws only when marked validated.
    Profile(DecoderProfile),
    /// Census and recording only.
    None,
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub input: CaptureInput,
    pub decoder: DecoderChoice,
    /// Write a `.pcap` of the game connection(s) here.
    pub record_dir: Option<PathBuf>,
    /// Keep what the AION 2 decoder learns (send radii, the movement field)
    /// in this JSON file for the next session.
    pub learned_path: Option<PathBuf>,
    /// Minimap measurements from the screen reader, if it runs.
    pub minimap: Option<Arc<MinimapLink>>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub running: bool,
    pub library: Option<String>,
    pub devices: Vec<String>,
    pub packets: u64,
    pub streams: StreamStats,
    pub census: Vec<OpcodeRow>,
    pub frames: u64,
    /// Which decoder runs ("AION 2 built-in", a profile name, or none).
    pub decoder: Option<String>,
    /// Whether decoded events reach the radar.
    pub decoder_active: bool,
    /// Built-in decoder details: own character, zone, live cubes, warnings.
    pub aion2: Option<Aion2Status>,
    /// Events decoded by the profile (sent to the radar only when validated).
    pub decoded: u64,
    pub decode_errors: u64,
    pub recording: Option<String>,
    pub recording_bytes: u64,
    pub markers_file: Option<String>,
    pub markers: usize,
    pub notes: Vec<String>,
    pub error: Option<String>,
}

pub type SharedStatus = Arc<Mutex<CaptureStatus>>;

fn lock(s: &SharedStatus) -> std::sync::MutexGuard<'_, CaptureStatus> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

/// Adds a marker to the current recording's marker file, stamped now.
pub fn add_marker(status: &SharedStatus, kind: &str) -> Result<usize, String> {
    let mut s = lock(status);
    let Some(path) = s.markers_file.clone() else {
        return Err("markers need an active capture with recording turned on".into());
    };
    append_marker(
        Path::new(&path),
        &Marker {
            t: now_ms() as f64,
            kind: kind.to_owned(),
            note: None,
        },
    )
    .map_err(|e| e.to_string())?;
    s.markers += 1;
    Ok(s.markers)
}

pub struct CaptureSource {
    cfg: CaptureConfig,
    status: SharedStatus,
}

enum Decoding {
    Aion2(Box<Aion2Decoder>),
    Profile(Decoder),
    None,
}

/// Everything after "a packet arrived", shared by live and file input.
struct Pipeline {
    streams: GameStreams,
    follow: MapConnection,
    census: Census,
    decoder: Decoding,
    send_events: bool,
    recorder: Option<Recorder>,
    frames: u64,
    packets: u64,
    learned_path: Option<PathBuf>,
    learned_saved: Option<crate::aion2::Aion2Learned>,
    minimap: Option<Arc<MinimapLink>>,
}

/// Raw packets (capture time, IP bytes) held back per flow direction.
type HeldPackets = HashMap<(SocketAddr, SocketAddr), Vec<(f64, Vec<u8>)>>;

struct Recorder {
    writer: PcapWriter<std::io::BufWriter<std::fs::File>>,
    pending: HeldPackets,
    full: bool,
}

impl Recorder {
    fn start(dir: &Path, status: &SharedStatus) -> std::io::Result<Recorder> {
        std::fs::create_dir_all(dir)?;
        let stamp = now_ms() / 1000;
        let pcap = dir.join(format!("aion2-capture-{stamp}.pcap"));
        let markers = dir.join(format!("aion2-capture-{stamp}.markers.jsonl"));
        let writer = PcapWriter::create(&pcap, linktype::RAW)?;
        let mut s = lock(status);
        s.recording = Some(pcap.display().to_string());
        s.markers_file = Some(markers.display().to_string());
        s.markers = 0;
        Ok(Recorder {
            writer,
            pending: HashMap::new(),
            full: false,
        })
    }

    /// Records game-connection packets only. Packets of flows still being
    /// probed are held back and written if the flow turns out to be the game.
    fn packet(&mut self, kind: FlowKind, key: (SocketAddr, SocketAddr), t_ms: f64, ip: &[u8]) {
        if self.full {
            return;
        }
        match kind {
            FlowKind::Game => {
                if let Some(held) = self.pending.remove(&key) {
                    for (t, p) in held {
                        let _ = self.writer.write(t, &p);
                    }
                }
                if let Err(e) = self.writer.write(t_ms, ip) {
                    log::error!("recording failed: {e}");
                    self.full = true;
                }
                if self.writer.bytes_written() > MAX_RECORDING_BYTES {
                    log::warn!("recording reached 1 GB and was stopped");
                    self.full = true;
                }
            }
            FlowKind::Probing => {
                if self.pending.len() > 512 && !self.pending.contains_key(&key) {
                    self.pending.clear(); // many short-lived non-game flows
                }
                let held = self.pending.entry(key).or_default();
                if held.len() < MAX_PENDING_PACKETS {
                    held.push((t_ms, ip.to_vec()));
                }
            }
            FlowKind::Ignored | FlowKind::Unknown => {
                self.pending.remove(&key);
            }
        }
    }
}

impl Pipeline {
    fn new(cfg: &CaptureConfig, status: &SharedStatus) -> Result<Pipeline, SourceError> {
        let invalid = |e: String| SourceError::Invalid(e);
        let (decoder, name, send_events) = match &cfg.decoder {
            DecoderChoice::Aion2(c) => {
                let d = Aion2Decoder::new((**c).clone()).map_err(invalid)?;
                let name = format!("AION 2 built-in ({})", c.client_build);
                (Decoding::Aion2(Box::new(d)), Some(name), true)
            }
            DecoderChoice::Profile(p) => {
                let d = Decoder::new(p.clone()).map_err(|e| invalid(e.to_string()))?;
                (Decoding::Profile(d), Some(p.name.clone()), p.validated)
            }
            DecoderChoice::None => (Decoding::None, None, false),
        };
        {
            let mut s = lock(status);
            s.decoder = name;
            s.decoder_active = send_events;
            s.notes.clear();
            match &decoder {
                Decoding::None => s
                    .notes
                    .push("No decoder: capturing for the packet census and recording only.".into()),
                Decoding::Profile(_) if !send_events => s.notes.push(
                    "The decoder profile is not marked validated: it decodes for diagnostics, \
                     but nothing is sent to the radar."
                        .into(),
                ),
                Decoding::Aion2(_) => s.notes.push(
                    "Hidden cubes are recognised in Verteron and Altgard. Other zones' cube \
                     templates are learned when you open a cube with a key. The server reports \
                     your position only with events (opening your wings, buffs); in between \
                     the radar follows you from objects entering and leaving range, and from \
                     your client's movement reports if it finds them."
                        .into(),
                ),
                Decoding::Profile(_) => {}
            }
        }
        let recorder = match &cfg.record_dir {
            Some(dir) => Some(Recorder::start(dir, status)?),
            None => None,
        };
        Ok(Pipeline {
            learned_path: cfg.learned_path.clone(),
            learned_saved: None,
            minimap: cfg.minimap.clone(),
            streams: GameStreams::new(),
            follow: MapConnection::new(),
            census: Census::new(),
            decoder,
            send_events,
            recorder,
            frames: 0,
            packets: 0,
        })
    }

    /// Handles one captured packet; `t_out` is the timestamp events carry.
    fn packet(
        &mut self,
        lt: u32,
        data: &[u8],
        t_ms: f64,
        t_out: Millis,
        emit: &mut Emit<'_>,
    ) -> bool {
        self.packets += 1;
        let Some(seg) = net::parse_frame(lt, data) else {
            return true;
        };
        let events = self.streams.push(&seg, t_ms);
        if let Some(rec) = &mut self.recorder {
            if let Some(ip) = net::ip_packet(lt, data) {
                let key = (seg.src, seg.dst);
                rec.packet(self.streams.kind(seg.src, seg.dst), key, t_ms, ip);
            }
        }
        let mut out: Vec<WireEvent> = Vec::new();
        for ev in events {
            match ev {
                StreamEvent::GameStream { conn, flow, .. } => {
                    // Recognising a connection changes nothing; the decoder
                    // follows whichever sends map data (see follow.rs).
                    log::info!("game stream {conn}: {flow}");
                }
                StreamEvent::Gap { conn, .. } => {
                    if let Decoding::Aion2(d) = &mut self.decoder {
                        if self.follow.follows(conn) {
                            out.extend(d.on_gap(t_out));
                        }
                    }
                }
                StreamEvent::ClientFrame { conn, frame, .. } => {
                    if let Decoding::Aion2(d) = &mut self.decoder {
                        if self.follow.follows(conn) {
                            out.extend(d.on_client_frame(&frame, t_out));
                        }
                    }
                }
                StreamEvent::Frame { conn, frame, .. } => {
                    self.frames += 1;
                    self.census.observe(&frame, t_ms);
                    match &mut self.decoder {
                        Decoding::Aion2(d) => out.extend(follow_frame(
                            &mut self.follow,
                            d,
                            conn,
                            frame.clone(),
                            t_ms,
                            t_out,
                        )),
                        Decoding::Profile(d) => out.extend(d.decode(&frame, t_out)),
                        Decoding::None => {}
                    }
                }
            }
        }
        if let Decoding::Aion2(d) = &mut self.decoder {
            out.extend(d.tick(t_out));
        }
        self.send(out, emit)
    }

    /// Minimap measurements queued by the screen reader.
    fn minimap(&mut self, emit: &mut Emit<'_>) -> bool {
        let Some(link) = self.minimap.clone() else {
            return true;
        };
        let samples = link.drain();
        let Decoding::Aion2(d) = &mut self.decoder else {
            return true;
        };
        let mut out = Vec::new();
        for s in &samples {
            out.extend(d.on_minimap(s));
        }
        link.set_view(d.minimap_view());
        self.send(out, emit)
    }

    /// Timers that run without packets (presence confirmations).
    fn tick(&mut self, t_out: Millis, emit: &mut Emit<'_>) -> bool {
        let out = match &mut self.decoder {
            Decoding::Aion2(d) => d.tick(t_out),
            _ => Vec::new(),
        };
        self.send(out, emit)
    }

    fn send(&mut self, events: Vec<WireEvent>, emit: &mut Emit<'_>) -> bool {
        if !self.send_events {
            return true;
        }
        events.into_iter().all(|e| emit(SourceMessage::Event(e)))
    }

    fn publish(&mut self, status: &SharedStatus) {
        if let Some(r) = &mut self.recorder {
            let _ = r.writer.flush();
        }
        if let Decoding::Profile(d) = &self.decoder {
            for op in d.opcodes() {
                self.census.mark_decoded(op);
            }
        }
        let mut s = lock(status);
        s.packets = self.packets;
        s.frames = self.frames;
        s.streams = self.streams.stats();
        s.census = self.census.rows().into_iter().take(60).collect();
        match &self.decoder {
            Decoding::Profile(d) => {
                s.decoded = d.decoded;
                s.decode_errors = d.field_errors;
            }
            Decoding::Aion2(d) => {
                let st = d.status(now_ms());
                s.decoded = st.cubes_seen_total;
                s.aion2 = Some(st);
            }
            Decoding::None => {}
        }
        if let Some(r) = &self.recorder {
            s.recording_bytes = r.writer.bytes_written();
        }
        drop(s);
        self.save_learned();
    }

    /// Writes the decoder's learned values when they changed.
    fn save_learned(&mut self) {
        let (Decoding::Aion2(d), Some(path)) = (&self.decoder, &self.learned_path) else {
            return;
        };
        let learned = d.learned();
        if learned.is_empty() || self.learned_saved.as_ref() == Some(&learned) {
            return;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&learned).expect("serialisable");
        match std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)) {
            Ok(()) => {
                log::info!("saved learned AION 2 values to {}", path.display());
                self.learned_saved = Some(learned);
            }
            Err(e) => log::warn!("could not save {}: {e}", path.display()),
        }
    }
}

impl CaptureSource {
    pub fn new(cfg: CaptureConfig) -> (CaptureSource, SharedStatus) {
        let status: SharedStatus = Arc::default();
        (
            CaptureSource {
                cfg,
                status: status.clone(),
            },
            status,
        )
    }

    fn run_live(
        &mut self,
        device: Option<String>,
        emit: &mut Emit<'_>,
        stop: &AtomicBool,
    ) -> Result<(), SourceError> {
        let api = PcapApi::load().map_err(|e| SourceError::Invalid(e.to_string()))?;
        let all = api
            .devices()
            .map_err(|e| SourceError::Invalid(e.to_string()))?;
        let chosen: Vec<_> = match &device {
            Some(d) => all.iter().filter(|x| &x.name == d).cloned().collect(),
            None => all.clone(),
        };
        if chosen.is_empty() {
            return Err(SourceError::Invalid(match device {
                Some(d) => format!("capture device {d:?} not found"),
                None => "no capture devices (is Npcap installed, and does this user have access?)"
                    .into(),
            }));
        }
        let mut handles = Vec::new();
        let mut open_errors = Vec::new();
        for d in &chosen {
            match api.open(&d.name) {
                Ok(h) => handles.push((h, d.label())),
                Err(e) => open_errors.push(e.to_string()),
            }
        }
        for e in &open_errors {
            log::warn!("{e}");
        }
        if handles.is_empty() {
            return Err(SourceError::Invalid(format!(
                "could not open any capture device: {}",
                open_errors.join("; ")
            )));
        }
        {
            let mut s = lock(&self.status);
            s.running = true;
            s.library = Some(api.version());
            s.devices = handles.iter().map(|(_, l)| l.clone()).collect();
        }
        let mut pipe = Pipeline::new(&self.cfg, &self.status)?;
        let detail = format!(
            "capturing on {} adapter(s){}",
            handles.len(),
            if pipe.send_events {
                ""
            } else {
                " — census only, nothing decoded for the radar"
            }
        );
        if !emit(SourceMessage::Status(SourceState::Connected, detail)) {
            return Ok(());
        }
        let mut last_status = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let mut got = 0;
            for (h, _) in handles.iter_mut() {
                let lt = h.linktype;
                for _ in 0..512 {
                    match h.next_packet() {
                        Ok(Next::Packet { t_ms, data }) => {
                            got += 1;
                            if !pipe.packet(lt, data, t_ms, t_ms as Millis, emit) {
                                return Ok(());
                            }
                        }
                        Ok(Next::Empty) => break,
                        Err(e) => {
                            log::warn!("{e}");
                            break;
                        }
                    }
                }
            }
            if !pipe.minimap(emit) {
                return Ok(());
            }
            if !pipe.tick(now_ms(), emit) {
                return Ok(());
            }
            if last_status.elapsed() >= STATUS_EVERY {
                pipe.publish(&self.status);
                last_status = Instant::now();
            }
            if got == 0 {
                std::thread::sleep(Duration::from_millis(4));
            }
        }
        pipe.publish(&self.status);
        Ok(())
    }

    fn run_file(
        &mut self,
        path: PathBuf,
        emit: &mut Emit<'_>,
        stop: &AtomicBool,
    ) -> Result<(), SourceError> {
        let records = pcapfile::read_file(&path)
            .map_err(|e| SourceError::Invalid(format!("{}: {e}", path.display())))?;
        lock(&self.status).running = true;
        let mut pipe = Pipeline::new(&self.cfg, &self.status)?;
        if !emit(SourceMessage::Status(
            SourceState::Connected,
            format!(
                "replaying {} packets from {}",
                records.len(),
                path.display()
            ),
        )) {
            return Ok(());
        }
        // Real-time playback, moved to the present so the radar accepts it.
        let t0 = records
            .iter()
            .map(|r| r.t_ms)
            .find(|t| *t > 0.0)
            .unwrap_or(0.0);
        let start_ms = now_ms();
        let start = Instant::now();
        let mut last_status = Instant::now();
        for r in &records {
            let offset = (r.t_ms - t0).max(0.0);
            let due = start + Duration::from_millis(offset as u64);
            while Instant::now() < due {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let played = start.elapsed().as_millis() as Millis;
                if !pipe.tick(start_ms + played, emit) {
                    return Ok(());
                }
                std::thread::sleep((due - Instant::now()).min(Duration::from_millis(20)));
            }
            if !pipe.packet(
                r.linktype,
                &r.data,
                r.t_ms,
                start_ms + offset as Millis,
                emit,
            ) {
                return Ok(());
            }
            if last_status.elapsed() >= STATUS_EVERY {
                pipe.publish(&self.status);
                last_status = Instant::now();
            }
        }
        pipe.publish(&self.status);
        emit(SourceMessage::Status(
            SourceState::Finished,
            "capture replay finished".into(),
        ));
        Ok(())
    }
}

impl DataSource for CaptureSource {
    fn name(&self) -> String {
        match &self.cfg.input {
            CaptureInput::Live { device: None } => "Packet capture (all adapters)".into(),
            CaptureInput::Live { device: Some(d) } => format!("Packet capture ({d})"),
            CaptureInput::File { path } => format!("Capture replay {}", path.display()),
        }
    }

    fn run(&mut self, emit: &mut Emit<'_>, stop: &AtomicBool) -> Result<(), SourceError> {
        let result = match self.cfg.input.clone() {
            CaptureInput::Live { device } => self.run_live(device, emit, stop),
            CaptureInput::File { path } => self.run_file(path, emit, stop),
        };
        let mut s = lock(&self.status);
        s.running = false;
        if let Err(e) = &result {
            s.error = Some(e.to_string());
        }
        result
    }
}

/// A frame of connection `conn` for the AION 2 decoder, if that is the
/// connection being followed; a newly followed one resets the decoder's
/// per-connection state (new entity ids) and is a zone change.
fn follow_frame(
    follow: &mut MapConnection,
    d: &mut Aion2Decoder,
    conn: u64,
    frame: crate::framing::Frame,
    t_ms: f64,
    t: Millis,
) -> Vec<WireEvent> {
    let kind = d.frame_kind(&frame);
    let routed = follow.route(conn, kind, frame, t_ms, t);
    let mut out = Vec::new();
    if routed.switched {
        d.on_new_connection();
        out.push(WireEvent::ZoneChanged {
            t,
            zone: format!("connection-{conn}"),
        });
    }
    for (f, ft) in &routed.frames {
        out.extend(d.on_frame(f, *ft));
    }
    out
}

/// Runs a capture file through framing (and a decoder) as fast as possible.
/// Used by the CLI and tests. Decoded events are returned whether or not a
/// profile is validated: this is the tool for checking one.
pub struct Offline {
    pub census: Census,
    pub samples: Vec<crate::discovery::Sample>,
    pub events: Vec<WireEvent>,
    pub streams: StreamStats,
    pub decoded: u64,
    pub decode_errors: u64,
    pub aion2: Option<Aion2Status>,
}

pub fn process_records(
    records: &[pcapfile::Record],
    decoder: DecoderChoice,
    keep_samples: bool,
) -> Result<Offline, String> {
    let mut streams = GameStreams::new();
    let mut census = Census::new();
    let mut decoder = match decoder {
        DecoderChoice::Aion2(c) => Decoding::Aion2(Box::new(Aion2Decoder::new(*c)?)),
        DecoderChoice::Profile(p) => Decoding::Profile(Decoder::new(p).map_err(|e| e.to_string())?),
        DecoderChoice::None => Decoding::None,
    };
    let mut samples = Vec::new();
    let mut events = Vec::new();
    let mut follow = MapConnection::new();
    let mut last_t = 0;
    for r in records {
        let Some(seg) = net::parse_frame(r.linktype, &r.data) else {
            continue;
        };
        let t = r.t_ms as Millis;
        last_t = t;
        for ev in streams.push(&seg, r.t_ms) {
            match ev {
                StreamEvent::GameStream { .. } => {}
                StreamEvent::Gap { conn, .. } => {
                    if let Decoding::Aion2(d) = &mut decoder {
                        if follow.follows(conn) {
                            events.extend(d.on_gap(t));
                        }
                    }
                }
                StreamEvent::ClientFrame { conn, frame, .. } => {
                    if let Decoding::Aion2(d) = &mut decoder {
                        if follow.follows(conn) {
                            events.extend(d.on_client_frame(&frame, t));
                        }
                    }
                }
                StreamEvent::Frame { conn, t_ms, frame } => {
                    census.observe(&frame, t_ms);
                    match &mut decoder {
                        Decoding::Aion2(d) => events.extend(follow_frame(
                            &mut follow,
                            d,
                            conn,
                            frame.clone(),
                            t_ms,
                            t,
                        )),
                        Decoding::Profile(d) => events.extend(d.decode(&frame, t)),
                        Decoding::None => {}
                    }
                    if keep_samples {
                        samples.push(crate::discovery::Sample {
                            t_ms,
                            conn,
                            opcode: frame.opcode,
                            payload: frame.payload,
                        });
                    }
                }
            }
        }
        if let Decoding::Aion2(d) = &mut decoder {
            events.extend(d.tick(t));
        }
    }
    let (decoded, decode_errors, aion2) = match &decoder {
        Decoding::Profile(d) => {
            for op in d.opcodes() {
                census.mark_decoded(op);
            }
            (d.decoded, d.field_errors, None)
        }
        Decoding::Aion2(d) => {
            let st = d.status(last_t);
            (st.cubes_seen_total, 0, Some(st))
        }
        Decoding::None => (0, 0, None),
    };
    Ok(Offline {
        census,
        samples,
        events,
        streams: streams.stats(),
        decoded,
        decode_errors,
        aion2,
    })
}
