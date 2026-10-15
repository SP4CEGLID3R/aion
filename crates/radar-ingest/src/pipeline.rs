//! The ingest pipeline: source thread → bounded channel → processing thread
//! (adapter + tracker) → frames at a fixed rate to a [`FrameSink`].
//!
//! The renderer never waits on a source, and a source never waits on the
//! renderer: the only shared state is the tracker behind a mutex held for
//! microseconds.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use radar_core::{
    CubeRecord, Diagnostics, HistoryEntry, RadarFrame, RadarSettings, SourceHealth, SourceState,
    StaleDisplay, Tracker,
};
use serde::Serialize;
use thiserror::Error;

use crate::adapter::EventAdapter;
use crate::clock::now_ms;
use crate::source::{self, DataSource, SourceMessage};

const CHANNEL_CAPACITY: usize = 8_192;

/// Receives frames. Called from the processing thread; must not block long.
pub trait FrameSink: Send + Sync + 'static {
    fn publish(&self, frame: &RadarFrame);
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("invalid coordinate transform: {0}")]
    Transform(#[from] radar_core::TransformError),
    #[error("could not start a thread: {0}")]
    Thread(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Display {
    stale_display: StaleDisplay,
    update_hz: u32,
}

struct Shared {
    tracker: Mutex<Tracker>,
    display: Mutex<Display>,
    health: Mutex<SourceHealth>,
    adapt_errors: AtomicU64,
}

/// Everything useful for debugging, as one serialisable report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugReport {
    pub generated_at: i64,
    pub source: SourceHealth,
    pub zone: Option<String>,
    pub diagnostics: Diagnostics,
    pub adapter_errors: u64,
    pub cubes: Vec<CubeRecord>,
    /// Newest last. For debugging only: never rendered as markers.
    pub history: Vec<HistoryEntry>,
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take the radar down with it.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Engine {
    /// Starts the source configured in `settings` (which should already be
    /// sanitized) and begins publishing frames to `sink`.
    pub fn start(
        settings: &RadarSettings,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Engine, EngineError> {
        let source = source::from_config(&settings.data_source);
        Self::start_with(settings, source, sink)
    }

    /// Like [`start`](Self::start) with an explicit source (or none).
    pub fn start_with(
        settings: &RadarSettings,
        source: Option<Box<dyn DataSource>>,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Engine, EngineError> {
        let adapter = EventAdapter::new(settings.coordinate_transform)?;
        let health = SourceHealth {
            name: source
                .as_ref()
                .map_or("No data source".into(), |s| s.name()),
            state: if source.is_some() {
                SourceState::Starting
            } else {
                SourceState::Disabled
            },
            detail: String::new(),
            last_event_at: None,
        };
        let shared = Arc::new(Shared {
            tracker: Mutex::new(Tracker::new(settings.tracker_config())),
            display: Mutex::new(Display {
                stale_display: settings.stale_display,
                update_hz: settings.update_hz.max(1),
            }),
            health: Mutex::new(health),
            adapt_errors: AtomicU64::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel::<SourceMessage>(CHANNEL_CAPACITY);
        let mut threads = Vec::new();

        if let Some(mut src) = source {
            let stop = stop.clone();
            let shared = shared.clone();
            threads.push(
                thread::Builder::new()
                    .name("radar-source".into())
                    .spawn(move || {
                        let name = src.name();
                        log::info!("data source starting: {name}");
                        let result = src.run(
                            &mut |msg| !stop.load(Ordering::Relaxed) && tx.send(msg).is_ok(),
                            &stop,
                        );
                        if let Err(e) = result {
                            log::error!("data source {name} failed: {e}");
                            // Through the channel, so it lands after any status the
                            // source sent before failing.
                            let msg = SourceMessage::Status(SourceState::Error, e.to_string());
                            if tx.send(msg).is_err() {
                                let mut h = lock(&shared.health);
                                h.state = SourceState::Error;
                                h.detail = e.to_string();
                            }
                        }
                        log::info!("data source stopped: {name}");
                    })?,
            );
        } else {
            drop(tx);
        }

        {
            let stop = stop.clone();
            let shared = shared.clone();
            threads.push(
                thread::Builder::new()
                    .name("radar-process".into())
                    .spawn(move || process(rx, adapter, shared, sink, stop))?,
            );
        }

        Ok(Engine {
            stop,
            shared,
            threads,
        })
    }

    /// Changes presentation settings without restarting the source.
    pub fn update_display(&self, stale_display: StaleDisplay, update_hz: u32) {
        *lock(&self.shared.display) = Display {
            stale_display,
            update_hz: update_hz.max(1),
        };
    }

    pub fn frame(&self) -> RadarFrame {
        let display = *lock(&self.shared.display);
        let mut frame = lock(&self.shared.tracker).frame(now_ms(), display.stale_display);
        frame.source = Some(lock(&self.shared.health).clone());
        frame
    }

    pub fn debug_report(&self) -> DebugReport {
        let tracker = lock(&self.shared.tracker);
        DebugReport {
            generated_at: now_ms(),
            source: lock(&self.shared.health).clone(),
            zone: tracker.zone().map(str::to_owned),
            diagnostics: tracker.diagnostics().clone(),
            adapter_errors: self.shared.adapt_errors.load(Ordering::Relaxed),
            cubes: tracker.cubes().cloned().collect(),
            history: tracker.history().cloned().collect(),
        }
    }

    /// Stops both threads and waits for them.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn process(
    rx: mpsc::Receiver<SourceMessage>,
    adapter: EventAdapter,
    shared: Arc<Shared>,
    sink: Arc<dyn FrameSink>,
    stop: Arc<AtomicBool>,
) {
    let mut next_tick = Instant::now();
    let mut source_gone = false;
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= next_tick {
            let display = *lock(&shared.display);
            let t = now_ms();
            let mut frame = {
                let mut tracker = lock(&shared.tracker);
                tracker.tick(t);
                tracker.frame(t, display.stale_display)
            };
            frame.source = Some(lock(&shared.health).clone());
            sink.publish(&frame);
            next_tick = now + Duration::from_secs_f64(1.0 / display.update_hz as f64);
            continue;
        }
        let wait = next_tick - now;
        if source_gone {
            thread::sleep(wait.min(Duration::from_millis(50)));
            continue;
        }
        match rx.recv_timeout(wait) {
            Ok(SourceMessage::Event(ev)) => match adapter.convert(ev) {
                Ok(event) => {
                    let t = now_ms();
                    lock(&shared.tracker).apply(event, t);
                    lock(&shared.health).last_event_at = Some(t);
                }
                Err(e) => {
                    let n = shared.adapt_errors.fetch_add(1, Ordering::Relaxed) + 1;
                    if n <= 10 || n.is_multiple_of(100) {
                        log::warn!("event rejected by adapter ({n} so far): {e}");
                    }
                }
            },
            Ok(SourceMessage::Status(state, detail)) => {
                log::info!("data source: {state:?} {detail}");
                let mut h = lock(&shared.health);
                h.state = state;
                h.detail = detail;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => source_gone = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Emit;
    use crate::wire::{WireEvent, WireId, WireProvenance};
    use radar_core::{CubeState, DataSourceConfig};

    #[derive(Default)]
    struct Collect(Mutex<Vec<RadarFrame>>);

    impl FrameSink for Collect {
        fn publish(&self, frame: &RadarFrame) {
            self.0.lock().unwrap().push(frame.clone());
        }
    }

    /// Emits a fixed list of events, then idles until stopped.
    struct Scripted(Vec<WireEvent>);

    impl DataSource for Scripted {
        fn name(&self) -> String {
            "scripted".into()
        }
        fn run(
            &mut self,
            emit: &mut Emit<'_>,
            stop: &AtomicBool,
        ) -> Result<(), source::SourceError> {
            emit(SourceMessage::Status(SourceState::Connected, "test".into()));
            for ev in self.0.drain(..) {
                if !emit(SourceMessage::Event(ev)) {
                    return Ok(());
                }
            }
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        }
    }

    fn wait_for(sink: &Collect, pred: impl Fn(&RadarFrame) -> bool) -> RadarFrame {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(f) = sink.0.lock().unwrap().iter().rev().find(|f| pred(f)) {
                return f.clone();
            }
            assert!(Instant::now() < deadline, "condition not met within 5 s");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn scripted_events_flow_through_to_frames() {
        let t = now_ms();
        let events = vec![
            WireEvent::Player {
                accuracy: None,
                height_accuracy: None,
                t,
                pos: [0.0; 3],
                heading: None,
                zone: None,
            },
            WireEvent::CubeSpawned {
                t,
                id: Some(WireId::Str("a".into())),
                pos: [3.0, 4.0, 0.0],
                label: None,
                zone: None,
                provenance: WireProvenance::Live,
            },
            WireEvent::SpawnPoint {
                t: None,
                id: Some(WireId::Str("sp".into())),
                pos: [1.0, 1.0, 0.0],
                label: None,
                zone: None,
            },
        ];
        let sink = Arc::new(Collect::default());
        let settings = RadarSettings {
            update_hz: 60,
            ..Default::default()
        };
        let engine =
            Engine::start_with(&settings, Some(Box::new(Scripted(events))), sink.clone()).unwrap();
        let f = wait_for(&sink, |f| f.stats.rejected >= 1 && !f.cubes.is_empty());
        assert_eq!(f.cubes.len(), 1);
        assert_eq!(f.cubes[0].id, "a");
        assert_eq!(f.cubes[0].state, CubeState::Spawned);
        assert!((f.cubes[0].distance.unwrap() - 5.0).abs() < 1e-9);
        assert_eq!(f.source.as_ref().unwrap().state, SourceState::Connected);

        let report = engine.debug_report();
        assert_eq!(
            report.diagnostics.not_live_by_provenance["static_spawn_point"],
            1
        );
        engine.stop();
    }

    #[test]
    fn no_source_still_publishes_empty_frames() {
        let sink = Arc::new(Collect::default());
        let settings = RadarSettings {
            data_source: DataSourceConfig::None,
            update_hz: 60,
            ..Default::default()
        };
        let engine = Engine::start(&settings, sink.clone()).unwrap();
        let f = wait_for(&sink, |_| true);
        assert!(f.cubes.is_empty());
        assert_eq!(f.source.unwrap().state, SourceState::Disabled);
        engine.stop();
    }

    #[test]
    fn a_failing_source_reports_an_error() {
        let sink = Arc::new(Collect::default());
        let settings = RadarSettings {
            data_source: DataSourceConfig::Replay {
                path: "/nonexistent/file.jsonl".into(),
                speed: 1.0,
                looped: false,
            },
            update_hz: 60,
            ..Default::default()
        };
        let engine = Engine::start(&settings, sink.clone()).unwrap();
        let f = wait_for(&sink, |f| {
            f.source.as_ref().unwrap().state == SourceState::Error
        });
        assert!(f.cubes.is_empty());
        engine.stop();
    }
}
