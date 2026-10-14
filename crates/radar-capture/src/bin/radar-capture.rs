//! `radar-capture`: record game traffic and find the cube packets.
//!
//! ```text
//! radar-capture devices
//! radar-capture record  [--device NAME] [--out DIR] [--seconds N]
//! radar-capture census  FILE.pcap
//! radar-capture analyze FILE.pcap [--markers FILE] [--kind cube_visible] [--before MS] [--after MS] [--top N]
//! radar-capture inspect FILE.pcap --opcode "XX YY" [--subtype XX] [--markers FILE] [--dump N]
//! radar-capture decode  FILE.pcap [--profile PROFILE.json | --config AION2.json] [--out EVENTS.jsonl]
//! radar-capture synth   OUT.pcap      (a made-up demo session + markers + profile)
//! ```
//!
//! FILE may be a `.pcap` or `.pcapng`, so Wireshark captures work too.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use radar_capture::aion2::Aion2Config;
use radar_capture::discovery::{
    self, correlate_by, correlation_table, hexdump, inspect, read_markers, Grouping, Marker,
};
use radar_capture::live::PcapApi;
use radar_capture::pcapfile::{self, PcapWriter};
use radar_capture::profile::{parse_hex, DecoderProfile};
use radar_capture::source::{
    add_marker, process_records, CaptureConfig, CaptureInput, CaptureSource, DecoderChoice,
};
use radar_capture::synthetic;
use radar_ingest::source::DataSource;
use radar_ingest::{SourceMessage, WireEvent};

struct Args {
    positional: Vec<String>,
    flags: HashMap<String, String>,
}

fn parse_args() -> Args {
    let mut positional = Vec::new();
    let mut flags = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            flags.insert(name.to_owned(), it.next().unwrap_or_default());
        } else {
            positional.push(a);
        }
    }
    Args { positional, flags }
}

impl Args {
    fn flag(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }
    fn num(&self, name: &str, default: f64) -> Result<f64, String> {
        match self.flag(name) {
            Some(v) => v.parse().map_err(|_| format!("--{name} must be a number")),
            None => Ok(default),
        }
    }
    fn file(&self, i: usize) -> Result<PathBuf, String> {
        self.positional
            .get(i)
            .map(PathBuf::from)
            .ok_or_else(|| "missing capture file argument".into())
    }
}

const USAGE: &str = "usage: radar-capture <devices|record|census|analyze|inspect|decode|synth> ...
  devices                              list capture adapters
  record [--device NAME] [--out DIR] [--seconds N]
                                       record the game connection to DIR (default ./recordings),
                                       printing hidden cubes as the built-in decoder finds them;
                                       type v + Enter when a cube appears, c + Enter when you
                                       collect one, q + Enter to stop
  census FILE                          packet types in a .pcap/.pcapng
  analyze FILE [--markers M] [--kind K] [--before MS] [--after MS] [--top N]
                                       rank packet types by how they line up with markers
  inspect FILE --opcode \"XX YY\" [--subtype XX] [--markers M] [--dump N]
                                       look for coordinate and id fields in one packet type
  decode FILE [--profile P | --config C] [--out OUT.jsonl]
                                       decode with the built-in AION 2 decoder (or a profile);
                                       OUT can be replayed in the radar
  synth OUT.pcap                       write a made-up demo session to practise on";

fn main() -> ExitCode {
    let args = parse_args();
    let Some(cmd) = args.positional.first().cloned() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest = Args {
        positional: args.positional[1..].to_vec(),
        flags: args.flags,
    };
    let result = match cmd.as_str() {
        "devices" => devices(),
        "record" => record(&rest),
        "census" => census(&rest),
        "analyze" => analyze(&rest),
        "inspect" => inspect_cmd(&rest),
        "decode" => decode(&rest),
        "synth" => synth(&rest),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn devices() -> Result<(), String> {
    let api = PcapApi::load().map_err(|e| e.to_string())?;
    println!("{}", api.version());
    for d in api.devices().map_err(|e| e.to_string())? {
        println!("{}\n    {}", d.name, d.label());
    }
    Ok(())
}

fn record(a: &Args) -> Result<(), String> {
    let dir = PathBuf::from(a.flag("out").unwrap_or("recordings"));
    let seconds = a.num("seconds", 0.0)?;
    let (mut src, status) = CaptureSource::new(CaptureConfig {
        input: CaptureInput::Live {
            device: a.flag("device").map(str::to_owned),
        },
        decoder: DecoderChoice::Aion2(Box::new(aion2_config(a)?)),
        record_dir: Some(dir),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let worker = std::thread::spawn(move || {
        src.run(
            &mut |m| {
                match m {
                    SourceMessage::Status(state, detail) => eprintln!("[{state:?}] {detail}"),
                    SourceMessage::Event(
                        e @ (WireEvent::CubeSpawned { .. }
                        | WireEvent::CubeCollected { .. }
                        | WireEvent::CubeLost { .. }
                        | WireEvent::CubeDespawned { .. }
                        | WireEvent::ZoneChanged { .. }),
                    ) => eprintln!("{}", e.to_line()),
                    SourceMessage::Event(_) => {}
                }
                true
            },
            &stop2,
        )
    });

    // Markers from stdin.
    let status2 = status.clone();
    let stop3 = stop.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let kind = match line.trim() {
                "q" => {
                    stop3.store(true, Ordering::Relaxed);
                    return;
                }
                "v" => "cube_visible",
                "c" => "cube_collected",
                "" => continue,
                _ => "note",
            };
            match add_marker(&status2, kind) {
                Ok(n) => eprintln!("marker {n}: {kind}"),
                Err(e) => eprintln!("{e}"),
            }
        }
    });

    let start = Instant::now();
    let mut last = Instant::now();
    while !worker.is_finished() {
        std::thread::sleep(Duration::from_millis(200));
        if seconds > 0.0 && start.elapsed().as_secs_f64() >= seconds {
            stop.store(true, Ordering::Relaxed);
        }
        if last.elapsed() >= Duration::from_secs(5) {
            last = Instant::now();
            let s = status.lock().unwrap();
            eprintln!(
                "{} packets, {} game frames, stream: {}, recording {} KB, {} markers",
                s.packets,
                s.frames,
                s.streams
                    .current_stream
                    .as_deref()
                    .unwrap_or("not found yet"),
                s.recording_bytes / 1024,
                s.markers
            );
        }
    }
    worker
        .join()
        .map_err(|_| "capture thread panicked".to_string())?
        .map_err(|e| e.to_string())?;
    let s = status.lock().unwrap();
    println!("recording: {}", s.recording.as_deref().unwrap_or("-"));
    println!("markers:   {}", s.markers_file.as_deref().unwrap_or("-"));
    Ok(())
}

fn load(a: &Args) -> Result<Vec<pcapfile::Record>, String> {
    let path = a.file(0)?;
    pcapfile::read_file(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn default_markers(a: &Args) -> Result<Option<PathBuf>, String> {
    if let Some(m) = a.flag("markers") {
        return Ok(Some(PathBuf::from(m)));
    }
    // recording.pcap → recording.markers.jsonl
    let f = a.file(0)?;
    let guess = f.with_extension("markers.jsonl");
    Ok(guess.exists().then_some(guess))
}

fn census(a: &Args) -> Result<(), String> {
    let off = process_records(&load(a)?, DecoderChoice::None, false)?;
    let st = &off.streams;
    println!(
        "game streams: {}   frames: {}   bundles: {} ({} bad)   resyncs: {}   dropped bytes: {}",
        st.game_streams,
        st.framing.frames,
        st.framing.bundles,
        st.framing.bad_bundles,
        st.framing.resyncs,
        st.framing.dropped_bytes
    );
    if st.game_streams == 0 {
        println!("No game stream was recognised: no heartbeat (0E 00 36) seen twice within 3 s.");
    }
    print!("{}", off.census.table(80));
    Ok(())
}

fn analyze(a: &Args) -> Result<(), String> {
    let path = default_markers(a)?.ok_or("no markers file (pass --markers)")?;
    let kind = a.flag("kind").unwrap_or("cube_visible");
    let markers: Vec<f64> = read_markers(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .into_iter()
        .filter(|m| m.kind == kind)
        .map(|m| m.t)
        .collect();
    if markers.is_empty() {
        return Err(format!("no {kind:?} markers in {}", path.display()));
    }
    let off = process_records(&load(a)?, DecoderChoice::None, true)?;
    let (before, after, top) = (
        a.num("before", 3000.0)?,
        a.num("after", 1000.0)?,
        a.num("top", 15.0)? as usize,
    );
    println!(
        "{} {kind} markers, {} frames. Best candidates first.\n\
         hit % = markers with this packet nearby; lift = how much more often than chance; score = both.",
        markers.len(),
        off.samples.len()
    );
    for (title, g) in [
        ("By opcode", Grouping::Opcode),
        ("By opcode / first byte after it", Grouping::Subtype),
    ] {
        println!("\n{title}:");
        print!(
            "{}",
            correlation_table(&correlate_by(&off.samples, &markers, before, after, g), top)
        );
    }
    println!(
        "\nNext: radar-capture inspect FILE --opcode \"XX YY\" [--subtype ZZ] on the top rows."
    );
    Ok(())
}

fn inspect_cmd(a: &Args) -> Result<(), String> {
    let op = a
        .flag("opcode")
        .and_then(parse_hex)
        .filter(|b| b.len() == 2)
        .ok_or("--opcode must be two hex bytes, e.g. \"41 36\"")?;
    let off = process_records(&load(a)?, DecoderChoice::None, true)?;
    let subtype = match a.flag("subtype") {
        Some(h) => Some(
            parse_hex(h)
                .filter(|b| b.len() == 1)
                .ok_or("--subtype must be one hex byte, e.g. 01")?[0],
        ),
        None => None,
    };
    let mine: Vec<&discovery::Sample> = off
        .samples
        .iter()
        .filter(|s| s.opcode[..] == op[..] && subtype.is_none_or(|b| s.payload.get(2) == Some(&b)))
        .collect();
    let payloads: Vec<&[u8]> = mine.iter().map(|s| s.payload.as_slice()).collect();
    print!("{}", inspect(&payloads));

    let dump = a.num("dump", 5.0)? as usize;
    let markers: Vec<Marker> = match default_markers(a)? {
        Some(p) => read_markers(&p).map_err(|e| e.to_string())?,
        None => Vec::new(),
    };
    let chosen: Vec<&&discovery::Sample> = if markers.is_empty() {
        mine.iter().take(dump).collect()
    } else {
        println!("\nPackets nearest to markers:");
        markers
            .iter()
            .filter_map(|m| {
                mine.iter()
                    .min_by(|x, y| (x.t_ms - m.t).abs().total_cmp(&(y.t_ms - m.t).abs()))
            })
            .take(dump)
            .collect()
    };
    for s in chosen {
        println!("\nt={:.0} ms, {} bytes", s.t_ms, s.payload.len());
        print!("{}", hexdump(&s.payload));
    }
    Ok(())
}

/// The built-in AION 2 decoder configuration, or `--config FILE`.
fn aion2_config(a: &Args) -> Result<Aion2Config, String> {
    match a.flag("config") {
        Some(p) => {
            Aion2Config::from_json(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?)
        }
        None => Ok(Aion2Config::builtin()),
    }
}

fn decode(a: &Args) -> Result<(), String> {
    let (choice, validated) = match a.flag("profile") {
        Some(path) => {
            let profile = DecoderProfile::load(Path::new(path)).map_err(|e| e.to_string())?;
            let v = profile.validated;
            (DecoderChoice::Profile(profile), v)
        }
        None => (DecoderChoice::Aion2(Box::new(aion2_config(a)?)), true),
    };
    let off = process_records(&load(a)?, choice, false)?;
    if let Some(st) = &off.aion2 {
        eprintln!(
            "AION 2 decoder: own character {:?}, zone {:?}, {} cube spawns, learned templates {:?}",
            st.self_id, st.zone, st.cubes_seen_total, st.learned_templates
        );
    }
    let mut out: Box<dyn Write> = match a.flag("out") {
        Some(p) => Box::new(std::fs::File::create(p).map_err(|e| e.to_string())?),
        None => Box::new(std::io::stdout()),
    };
    for ev in &off.events {
        writeln!(out, "{}", ev.to_line()).map_err(|e| e.to_string())?;
    }
    eprintln!(
        "{} events decoded, {} packets of profiled types could not be read{}",
        off.decoded,
        off.decode_errors,
        if validated {
            ""
        } else {
            " (profile not yet marked validated)"
        }
    );
    Ok(())
}

fn synth(a: &Args) -> Result<(), String> {
    let out = a.file(0)?;
    let t0 = radar_ingest::clock::now_ms() as f64;
    let s = synthetic::session(t0, 120);
    let mut w =
        PcapWriter::create(&out, radar_capture::net::linktype::RAW).map_err(|e| e.to_string())?;
    for r in &s.records {
        w.write(r.t_ms, &r.data).map_err(|e| e.to_string())?;
    }
    w.flush().map_err(|e| e.to_string())?;
    let markers = out.with_extension("markers.jsonl");
    let _ = std::fs::remove_file(&markers);
    for m in &s.markers {
        discovery::append_marker(&markers, m).map_err(|e| e.to_string())?;
    }
    let profile = out.with_extension("profile.json");
    std::fs::write(&profile, synthetic::PROFILE_JSON).map_err(|e| e.to_string())?;
    println!("SYNTHETIC demo (invented opcodes, not AION 2):");
    println!(
        "  capture: {}\n  markers: {}\n  profile: {}",
        out.display(),
        markers.display(),
        profile.display()
    );
    Ok(())
}
