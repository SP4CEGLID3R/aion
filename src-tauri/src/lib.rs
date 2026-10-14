//! Tauri shell: windows, settings persistence, and the bridge between the
//! ingest pipeline and the webviews. All radar logic lives in `radar-core`,
//! `radar-ingest` and `radar-capture`; this crate only wires them to the
//! desktop.

mod store;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use radar_capture::aion2::North;
use radar_capture::live::{Device, PcapApi};
use radar_capture::{
    Aion2Config, CaptureConfig, CaptureInput, CaptureSource, CaptureStatus, DecoderChoice,
    DecoderProfile, SharedStatus,
};
use radar_core::{
    CaptureDecoder, CaptureNorth, DataSourceConfig, RadarFrame, RadarSettings, WindowMode,
};
use radar_ingest::source::DataSource;
use radar_ingest::{DebugReport, Engine, FrameSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use store::SettingsStore;

const RADAR: &str = "radar";
const SETTINGS: &str = "settings";
const FRAME_EVENT: &str = "radar-frame";
const SETTINGS_EVENT: &str = "settings-changed";
const MARKER_EVENT: &str = "capture-marker";

struct Inner {
    settings: RadarSettings,
    engine: Option<Engine>,
    /// Status of the packet-capture source, when that is the source.
    capture: Option<SharedStatus>,
}

struct AppState {
    inner: Mutex<Inner>,
    store: SettingsStore,
    paths: Paths,
}

impl AppState {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Pushes frames to the radar window only; the settings window polls.
struct TauriSink(AppHandle);

impl FrameSink for TauriSink {
    fn publish(&self, frame: &RadarFrame) {
        // Fails only while the window is being created or closed.
        let _ = self.0.emit_to(RADAR, FRAME_EVENT, frame);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsUpdate {
    settings: RadarSettings,
    warnings: Vec<String>,
}

struct Started {
    engine: Option<Engine>,
    capture: Option<SharedStatus>,
    warnings: Vec<String>,
}

/// Builds the packet-capture source. A missing or broken profile is not
/// fatal: capture still runs for the census and recording, and draws nothing.
fn capture_source(
    cfg: &DataSourceConfig,
    paths: &Paths,
    warnings: &mut Vec<String>,
) -> Option<(Box<dyn DataSource>, SharedStatus)> {
    let DataSourceConfig::PacketCapture {
        device,
        file,
        decoder,
        profile_path,
        north,
        include_unconfirmed,
        record,
    } = cfg
    else {
        return None;
    };
    let decoder = match decoder {
        CaptureDecoder::Aion2 => {
            let mut config = aion2_config(&paths.config_dir, warnings);
            config.use_hypotheses |= *include_unconfirmed;
            config.north = match north {
                CaptureNorth::NegY => North::NegY,
                CaptureNorth::PosY => North::PosY,
            };
            DecoderChoice::Aion2(Box::new(config))
        }
        CaptureDecoder::Profile => match profile_path.as_deref() {
            None => {
                warnings.push("no decoder profile chosen; capturing without one".into());
                DecoderChoice::None
            }
            Some(p) => match DecoderProfile::load(Path::new(p)) {
                Ok(profile) => {
                    if !profile.validated {
                        warnings.push(format!(
                            "decoder profile \"{}\" is not marked validated: nothing will be drawn from it",
                            profile.name
                        ));
                    }
                    DecoderChoice::Profile(profile)
                }
                Err(e) => {
                    warnings.push(format!(
                        "decoder profile not loaded ({e}); capturing without one"
                    ));
                    DecoderChoice::None
                }
            },
        },
        CaptureDecoder::None => DecoderChoice::None,
    };
    let input = match file {
        Some(path) => CaptureInput::File { path: path.into() },
        None => CaptureInput::Live {
            device: device.clone(),
        },
    };
    let (source, status) = CaptureSource::new(CaptureConfig {
        input,
        decoder,
        record_dir: record.then(|| paths.recordings.clone()),
    });
    Some((Box::new(source), status))
}

/// The built-in AION 2 decoder configuration, replaced by
/// `aion2-decoder.json` in the config folder when present (after a patch).
fn aion2_config(config_dir: &Path, warnings: &mut Vec<String>) -> Aion2Config {
    let path = config_dir.join("aion2-decoder.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => match Aion2Config::from_json(&text) {
            Ok(c) => {
                log::info!("using AION 2 decoder config {}", path.display());
                c
            }
            Err(e) => {
                warnings.push(format!(
                    "{} is invalid ({e}); using the built-in one",
                    path.display()
                ));
                Aion2Config::builtin()
            }
        },
        Err(_) => Aion2Config::builtin(),
    }
}

/// Where the app keeps its files.
struct Paths {
    recordings: PathBuf,
    config_dir: PathBuf,
}

fn start_engine(app: &AppHandle, settings: &RadarSettings, paths: &Paths) -> Started {
    let sink = Arc::new(TauriSink(app.clone()));
    let mut warnings = Vec::new();
    let (result, capture) = match capture_source(&settings.data_source, paths, &mut warnings) {
        Some((source, status)) => (
            Engine::start_with(settings, Some(source), sink),
            Some(status),
        ),
        None => (Engine::start(settings, sink), None),
    };
    match result {
        Ok(engine) => Started {
            engine: Some(engine),
            capture,
            warnings,
        },
        Err(e) => {
            log::error!("could not start the radar pipeline: {e}");
            warnings.push(format!("radar pipeline did not start: {e}"));
            Started {
                engine: None,
                capture: None,
                warnings,
            }
        }
    }
}

/// Whether a change needs the source and tracker rebuilt, or only the display.
fn needs_restart(old: &RadarSettings, new: &RadarSettings) -> bool {
    old.data_source != new.data_source
        || old.coordinate_transform != new.coordinate_transform
        || old.stale_timeout_secs != new.stale_timeout_secs
        || old.stale_retain_secs != new.stale_retain_secs
}

fn apply_window_mode(window: &WebviewWindow, s: &RadarSettings) {
    let overlay = s.window_mode == WindowMode::Overlay;
    let results = [
        window.set_decorations(!overlay),
        window.set_always_on_top(overlay),
        window.set_shadow(!overlay),
        window.set_ignore_cursor_events(overlay && s.click_through),
    ];
    for r in results {
        if let Err(e) = r {
            log::warn!("window mode change failed: {e}");
        }
    }
}

fn create_radar_window(app: &AppHandle, s: &RadarSettings) -> tauri::Result<WebviewWindow> {
    let overlay = s.window_mode == WindowMode::Overlay;
    let window = WebviewWindowBuilder::new(app, RADAR, WebviewUrl::App("index.html".into()))
        .title("Cube Radar")
        .inner_size(320.0, 400.0)
        .min_inner_size(160.0, 200.0)
        .resizable(true)
        // Transparency is fixed at creation; separate mode paints an opaque
        // background instead of toggling it.
        .transparent(true)
        .decorations(!overlay)
        .always_on_top(overlay)
        .shadow(!overlay)
        // Do not take the keyboard from the game when the overlay appears.
        .focused(false)
        .build()?;
    apply_window_mode(&window, s);
    Ok(window)
}

fn show_settings_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(SETTINGS) {
        w.unminimize()?;
        w.show()?;
        return w.set_focus();
    }
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("Cube Radar — Settings")
        .inner_size(720.0, 760.0)
        .min_inner_size(420.0, 360.0)
        .build()?;
    Ok(())
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> RadarSettings {
    state.lock().settings.clone()
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: RadarSettings,
) -> SettingsUpdate {
    let (settings, mut warnings) = settings.sanitized();
    {
        let mut inner = state.lock();
        if needs_restart(&inner.settings, &settings) || inner.engine.is_none() {
            // Stop first: a TCP source must release its port before rebinding.
            if let Some(old) = inner.engine.take() {
                old.stop();
            }
            let started = start_engine(&app, &settings, &state.paths);
            inner.engine = started.engine;
            inner.capture = started.capture;
            warnings.extend(started.warnings);
        } else if let Some(engine) = &inner.engine {
            engine.update_display(settings.stale_display, settings.update_hz);
        }
        inner.settings = settings.clone();
    }
    if let Err(e) = state.store.save(&settings) {
        log::error!("could not save settings: {e}");
        warnings.push(format!("settings applied but not saved: {e}"));
    }
    if let Some(w) = app.get_webview_window(RADAR) {
        apply_window_mode(&w, &settings);
    }
    let _ = app.emit(SETTINGS_EVENT, &settings);
    for w in &warnings {
        log::warn!("settings: {w}");
    }
    SettingsUpdate { settings, warnings }
}

#[tauri::command]
fn reset_settings(app: AppHandle, state: State<'_, AppState>) -> SettingsUpdate {
    update_settings(app, state, RadarSettings::default())
}

#[tauri::command]
fn get_debug_report(state: State<'_, AppState>) -> Option<DebugReport> {
    state.lock().engine.as_ref().map(Engine::debug_report)
}

#[tauri::command]
fn open_settings_window(app: AppHandle) -> Result<(), String> {
    show_settings_window(&app).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_capture_devices() -> Result<Vec<Device>, String> {
    let api = PcapApi::load().map_err(|e| e.to_string())?;
    api.devices().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_capture_status(state: State<'_, AppState>) -> Option<CaptureStatus> {
    let status = state.lock().capture.clone()?;
    let s = status.lock().unwrap_or_else(|p| p.into_inner()).clone();
    Some(s)
}

#[tauri::command]
fn add_capture_marker(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: String,
) -> Result<usize, String> {
    mark(&app, &state, &kind)
}

#[tauri::command]
fn get_recordings_dir(state: State<'_, AppState>) -> String {
    state.paths.recordings.display().to_string()
}

fn mark(app: &AppHandle, state: &AppState, kind: &str) -> Result<usize, String> {
    let status = state
        .lock()
        .capture
        .clone()
        .ok_or("markers need the packet-capture source with recording on")?;
    let result = radar_capture::add_marker(&status, kind);
    let _ = app.emit(
        MARKER_EVENT,
        match &result {
            Ok(n) => format!("Marker {n}: {}", kind.replace('_', " ")),
            Err(e) => format!("Marker not saved: {e}"),
        },
    );
    result
}

/// Ctrl+Alt+V: a cube appeared. Ctrl+Alt+C: a cube was collected. Global, so
/// they work while the game has focus.
fn marker_shortcuts() -> [(Shortcut, &'static str); 2] {
    let mods = Some(Modifiers::CONTROL | Modifiers::ALT);
    [
        (Shortcut::new(mods, Code::KeyV), "cube_visible"),
        (Shortcut::new(mods, Code::KeyC), "cube_collected"),
    ]
}

pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let Some((_, kind)) =
                        marker_shortcuts().into_iter().find(|(s, _)| s == shortcut)
                    else {
                        return;
                    };
                    if let Some(state) = app.try_state::<AppState>() {
                        if let Err(e) = mark(app, &state, kind) {
                            log::warn!("marker: {e}");
                        }
                    }
                })
                .build(),
        )
        .plugin(
            tauri_plugin_log::Builder::new()
                .clear_targets()
                .level(log::LevelFilter::Info)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ))
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("radar".into()),
                    },
                ))
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            let dir = app.path().app_config_dir()?;
            let store = SettingsStore::new(&dir);
            let loaded = store.load();
            for w in &loaded.warnings {
                log::warn!("settings: {w}");
            }
            log::info!("settings file: {}", store.path().display());
            if loaded.first_run {
                if let Err(e) = store.save(&loaded.settings) {
                    log::warn!("could not write default settings: {e}");
                }
            }

            let paths = Paths {
                recordings: app.path().app_data_dir()?.join("recordings"),
                config_dir: dir.clone(),
            };
            let started = start_engine(&handle, &loaded.settings, &paths);
            for w in &started.warnings {
                log::warn!("{w}");
            }
            let settings = loaded.settings.clone();
            // State first: the windows' scripts call commands as soon as they load.
            app.manage(AppState {
                inner: Mutex::new(Inner {
                    settings: loaded.settings,
                    engine: started.engine,
                    capture: started.capture,
                }),
                store,
                paths,
            });
            for (shortcut, kind) in marker_shortcuts() {
                // Another program may own the key; markers still work from the settings window.
                if let Err(e) = app.global_shortcut().register(shortcut) {
                    log::warn!("could not register the {kind} hotkey: {e}");
                }
            }
            create_radar_window(&handle, &settings)?;
            if loaded.first_run {
                show_settings_window(&handle)?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the radar quits the app; the settings window just closes.
            if window.label() == RADAR {
                if let tauri::WindowEvent::Destroyed = event {
                    let app = window.app_handle();
                    if let Some(state) = app.try_state::<AppState>() {
                        if let Some(engine) = state.lock().engine.take() {
                            engine.stop();
                        }
                    }
                    app.exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            update_settings,
            reset_settings,
            get_debug_report,
            open_settings_window,
            list_capture_devices,
            get_capture_status,
            add_capture_marker,
            get_recordings_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running the radar");
}

#[cfg(test)]
mod tests {
    use super::*;
    use radar_core::{DataSourceConfig, StaleDisplay};

    #[test]
    fn display_only_changes_do_not_restart_the_source() {
        let old = RadarSettings::default();
        let display = RadarSettings {
            range: 999.0,
            opacity: 0.5,
            stale_display: StaleDisplay::MarkStale,
            window_mode: WindowMode::Separate,
            ..old.clone()
        };
        assert!(!needs_restart(&old, &display));

        let source = RadarSettings {
            data_source: DataSourceConfig::LocalTcp { port: 50_000 },
            ..old.clone()
        };
        assert!(needs_restart(&old, &source));

        let timeout = RadarSettings {
            stale_timeout_secs: 3.0,
            ..old.clone()
        };
        assert!(needs_restart(&old, &timeout));
    }
}
