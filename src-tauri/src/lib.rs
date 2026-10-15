//! Tauri shell: windows, settings persistence, and the bridge between the
//! ingest pipeline and the webviews. All radar logic lives in `radar-core`,
//! `radar-ingest` and `radar-capture`; this crate only wires them to the
//! desktop.

mod minimap;
mod store;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use radar_capture::aion2::{Aion2Learned, North};
use radar_capture::live::{Device, PcapApi};
use radar_capture::minimap::{MinimapLink, MinimapView};
use radar_capture::{
    Aion2Config, CaptureConfig, CaptureInput, CaptureSource, CaptureStatus, DecoderChoice,
    DecoderProfile, SharedStatus,
};
use radar_core::{
    CaptureDecoder, CaptureNorth, DataSourceConfig, MinimapSettings, RadarFrame, RadarSettings,
    ScreenRegion, WindowMode,
};
use radar_ingest::source::DataSource;
use radar_ingest::{DebugReport, Engine, FrameSink};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use minimap::MinimapService;
use store::SettingsStore;

const RADAR: &str = "radar";
const SETTINGS: &str = "settings";
/// The arrow around the character, over the game.
const GUIDE: &str = "guide";
/// The guide window's width and height as multiples of the ring's width.
const GUIDE_AREA: (f64, f64) = (2.2, 1.3);
const FRAME_EVENT: &str = "radar-frame";
const SETTINGS_EVENT: &str = "settings-changed";
const MARKER_EVENT: &str = "capture-marker";
/// Largest sound file the alert will load.
const MAX_SOUND_BYTES: u64 = 10 * 1024 * 1024;

struct Inner {
    settings: RadarSettings,
    engine: Option<Engine>,
    /// Status of the packet-capture source, when that is the source.
    capture: Option<SharedStatus>,
    /// The minimap reader, when it runs.
    minimap: Option<MinimapService>,
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

/// Pushes frames to the radar and the guide arrow; the settings window polls.
struct TauriSink(AppHandle);

impl FrameSink for TauriSink {
    fn publish(&self, frame: &RadarFrame) {
        // Fails only while a window is being created or closed.
        let _ = self.0.emit_to(RADAR, FRAME_EVENT, frame);
        if self.0.get_webview_window(GUIDE).is_some() {
            let _ = self.0.emit_to(GUIDE, FRAME_EVENT, frame);
        }
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
    minimap: Option<MinimapService>,
    warnings: Vec<String>,
}

/// Builds the packet-capture source. A missing or broken profile is not
/// fatal: capture still runs for the census and recording, and draws nothing.
fn capture_source(
    cfg: &DataSourceConfig,
    paths: &Paths,
    warnings: &mut Vec<String>,
    minimap: Option<Arc<MinimapLink>>,
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
            config.apply_learned(&aion2_learned(&paths.config_dir));
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
        learned_path: Some(paths.config_dir.join(LEARNED_FILE)),
        minimap,
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

/// What the decoder learned in game (send radii, the movement field).
const LEARNED_FILE: &str = "aion2-learned.json";

fn aion2_learned(config_dir: &Path) -> Aion2Learned {
    let path = config_dir.join(LEARNED_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("ignoring {}: {e}", path.display());
            Aion2Learned::default()
        }),
        Err(_) => Aion2Learned::default(),
    }
}

/// Where the app keeps its files.
struct Paths {
    recordings: PathBuf,
    config_dir: PathBuf,
}

/// Minimap tracking needs live packet capture with the AION 2 decoder: the
/// minimap only gives movement, the server fixes say where it starts.
fn wants_minimap(s: &RadarSettings) -> bool {
    s.minimap.enabled
        && matches!(
            &s.data_source,
            DataSourceConfig::PacketCapture {
                decoder: CaptureDecoder::Aion2,
                file: None,
                ..
            }
        )
}

fn monitor_rect(m: &Monitor) -> (i32, i32, u32, u32) {
    let (p, s) = (m.position(), m.size());
    (p.x, p.y, s.width, s.height)
}

fn main_monitor(app: &AppHandle) -> Option<Monitor> {
    app.primary_monitor()
        .ok()
        .flatten()
        .or_else(|| app.available_monitors().ok()?.into_iter().next())
}

fn monitor_at(app: &AppHandle, x: i32, y: i32) -> Option<Monitor> {
    app.available_monitors().ok()?.into_iter().find(|m| {
        let (mx, my, mw, mh) = monitor_rect(m);
        x >= mx && y >= my && x < mx + mw as i32 && y < my + mh as i32
    })
}

/// (region, is the default spot, UI scale) for the minimap reader.
fn minimap_region(app: &AppHandle, m: &MinimapSettings) -> Option<(ScreenRegion, bool, f64)> {
    match m.region {
        Some(r) => {
            let (cx, cy) = (r.x + r.width as i32 / 2, r.y + r.height as i32 / 2);
            let scale = monitor_at(app, cx, cy)
                .or_else(|| main_monitor(app))
                .map(|mon| minimap::ui_scale(mon.size().height))
                .unwrap_or(r.height as f64 / 206.0);
            Some((r, false, scale))
        }
        None => {
            let (x, y, w, h) = monitor_rect(&main_monitor(app)?);
            Some((
                minimap::default_region(x, y, w, h),
                true,
                minimap::ui_scale(h),
            ))
        }
    }
}

fn start_engine(app: &AppHandle, settings: &RadarSettings, paths: &Paths) -> Started {
    let sink = Arc::new(TauriSink(app.clone()));
    let mut warnings = Vec::new();
    let link = wants_minimap(settings).then(|| Arc::new(MinimapLink::new()));
    let (result, capture) =
        match capture_source(&settings.data_source, paths, &mut warnings, link.clone()) {
            Some((source, status)) => (
                Engine::start_with(settings, Some(source), sink),
                Some(status),
            ),
            None => (Engine::start(settings, sink), None),
        };
    let minimap = link.and_then(|link| match minimap_region(app, &settings.minimap) {
        Some((region, default, scale)) => Some(MinimapService::start(
            region,
            default,
            scale,
            settings.minimap.fps,
            link,
        )),
        None => {
            warnings.push("minimap tracking: no monitor found".into());
            None
        }
    });
    match result {
        Ok(engine) => Started {
            engine: Some(engine),
            capture,
            minimap,
            warnings,
        },
        Err(e) => {
            log::error!("could not start the radar pipeline: {e}");
            warnings.push(format!("radar pipeline did not start: {e}"));
            Started {
                engine: None,
                capture: None,
                minimap: None,
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
        || old.minimap.enabled != new.minimap.enabled
        || old.minimap.region != new.minimap.region
        || old.minimap.fps != new.minimap.fps
}

/// While the minimap is read from the screen, the radar and the guide arrow
/// keep out of screen captures (WDA_EXCLUDEFROMCAPTURE), so placing them
/// over the minimap cannot disturb the tracking. They then also do not show
/// in recordings and screenshots.
fn apply_protection(app: &AppHandle, s: &RadarSettings) {
    let on = wants_minimap(s);
    for label in [RADAR, GUIDE] {
        if let Some(w) = app.get_webview_window(label) {
            if let Err(e) = w.set_content_protected(on) {
                log::warn!("content protection: {e}");
            }
        }
    }
}

/// The monitor the game (its minimap) is on.
fn game_monitor(app: &AppHandle, s: &RadarSettings) -> Option<Monitor> {
    match minimap_region(app, &s.minimap) {
        Some((r, _, _)) => monitor_at(app, r.x + r.width as i32 / 2, r.y + r.height as i32 / 2)
            .or_else(|| main_monitor(app)),
        None => main_monitor(app),
    }
}

/// Shows, moves or closes the arrow window around the character. Not from
/// a synchronous command (window creation deadlocks there on Windows).
fn sync_guide_window(app: &AppHandle, s: &RadarSettings) {
    let want = s.guide.screen_arrow && wants_minimap(s);
    let existing = app.get_webview_window(GUIDE);
    if !want {
        if let Some(w) = existing {
            let _ = w.close();
        }
        return;
    }
    let Some(mon) = game_monitor(app, s) else {
        return;
    };
    let (mx, my, mw, mh) = monitor_rect(&mon);
    // The ring is `screen_size` wide; the window leaves room around it for
    // the labels (src/guide.ts draws to the same proportions).
    let size = s.guide.screen_size;
    let (w_px, h_px) = (
        (size * GUIDE_AREA.0).round() as u32,
        (size * GUIDE_AREA.1).round() as u32,
    );
    let x = mx + (s.guide.screen_x * mw as f64 - w_px as f64 / 2.0).round() as i32;
    let y = my + (s.guide.screen_y * mh as f64 - h_px as f64 / 2.0).round() as i32;
    let scale = mon.scale_factor().max(0.1);
    let window = match existing {
        Some(w) => w,
        None => {
            let built = WebviewWindowBuilder::new(app, GUIDE, WebviewUrl::App("guide.html".into()))
                .title("Cube guide")
                .inner_size(w_px as f64 / scale, h_px as f64 / scale)
                .position(x as f64 / scale, y as f64 / scale)
                .transparent(true)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                // Resizable so later size changes apply (GTK ignores them on
                // fixed-size windows); clicks pass through it anyway.
                .resizable(true)
                .shadow(false)
                .focused(false)
                .content_protected(true)
                .build();
            match built {
                Ok(w) => w,
                Err(e) => {
                    log::warn!("guide window: {e}");
                    return;
                }
            }
        }
    };
    let results = [
        window.set_size(PhysicalSize::new(w_px, h_px)),
        window.set_position(PhysicalPosition::new(x, y)),
        window.set_ignore_cursor_events(true),
    ];
    for r in results {
        if let Err(e) = r {
            log::warn!("guide window: {e}");
        }
    }
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
        .content_protected(wants_minimap(s))
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

/// Async: applying settings may open the guide window.
#[tauri::command]
async fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: RadarSettings,
) -> Result<SettingsUpdate, String> {
    Ok(apply_settings(&app, &state, settings))
}

#[tauri::command]
async fn reset_settings(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SettingsUpdate, String> {
    Ok(apply_settings(&app, &state, RadarSettings::default()))
}

fn apply_settings(app: &AppHandle, state: &AppState, settings: RadarSettings) -> SettingsUpdate {
    let (settings, mut warnings) = settings.sanitized();
    {
        let mut inner = state.lock();
        if needs_restart(&inner.settings, &settings) || inner.engine.is_none() {
            // Stop first: a TCP source must release its port before rebinding.
            if let Some(old) = inner.engine.take() {
                old.stop();
            }
            if let Some(old) = inner.minimap.take() {
                old.stop();
            }
            let started = start_engine(app, &settings, &state.paths);
            inner.engine = started.engine;
            inner.capture = started.capture;
            inner.minimap = started.minimap;
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
    apply_protection(app, &settings);
    sync_guide_window(app, &settings);
    let _ = app.emit(SETTINGS_EVENT, &settings);
    for w in &warnings {
        log::warn!("settings: {w}");
    }
    SettingsUpdate { settings, warnings }
}

#[tauri::command]
fn get_debug_report(state: State<'_, AppState>) -> Option<DebugReport> {
    state.lock().engine.as_ref().map(Engine::debug_report)
}

/// Async on purpose: creating a window from a synchronous command deadlocks
/// on Windows (WebView2 waits for the main thread the command is blocking).
#[tauri::command]
async fn open_settings_window(app: AppHandle) -> Result<(), String> {
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

/// The bytes of a sound file the user chose for the cube alert. Only audio
/// files, and only up to `MAX_SOUND_BYTES`.
#[tauri::command]
fn read_sound_file(path: String) -> Result<tauri::ipc::Response, String> {
    let p = Path::new(path.trim());
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(ext.as_str(), "wav" | "mp3" | "ogg" | "flac" | "m4a") {
        return Err("choose a .wav, .mp3, .ogg, .flac or .m4a file".into());
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !meta.is_file() || meta.len() > MAX_SOUND_BYTES {
        return Err(format!(
            "{} is not a sound file of at most 10 MB",
            p.display()
        ));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
fn get_recordings_dir(state: State<'_, AppState>) -> String {
    state.paths.recordings.display().to_string()
}

#[tauri::command]
fn get_minimap_status(state: State<'_, AppState>) -> Option<minimap::VisionStatus> {
    state.lock().minimap.as_ref().map(|m| m.shared.status())
}

/// The decoder's view of the minimap: scale, tracking state, camera.
#[tauri::command]
fn get_minimap_view(state: State<'_, AppState>) -> Option<MinimapView> {
    state.lock().minimap.as_ref().map(|m| m.link.view())
}

/// The latest minimap picture if newer than `after` (see
/// `minimap::Shared::frame_after` for the layout); empty otherwise.
#[tauri::command]
fn minimap_frame(state: State<'_, AppState>, after: u64) -> tauri::ipc::Response {
    let parts = {
        let inner = state.lock();
        inner
            .minimap
            .as_ref()
            .map(|m| (m.shared.clone(), m.link.clone()))
    };
    let Some((shared, link)) = parts else {
        return tauri::ipc::Response::new(Vec::new());
    };
    let view = link.view();
    tauri::ipc::Response::new(shared.frame_after(after, view.px_per_m, view.calibrated))
}

/// Puts a shortcut to this app on the desktop (Windows: a normal .lnk, made
/// with the Windows Script Host). Returns where it was written.
#[tauri::command]
async fn create_desktop_shortcut(app: AppHandle) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let desktop = app.path().desktop_dir().map_err(|e| e.to_string())?;
    let link = desktop.join("AION 2 Cube Radar.lnk");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // PowerShell single-quoted strings: double any quote inside.
        let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
        let dir: &Path = exe.parent().unwrap_or(exe.as_path());
        let script = format!(
            "$s = (New-Object -ComObject WScript.Shell).CreateShortcut({}); \
             $s.TargetPath = {}; $s.WorkingDirectory = {}; $s.IconLocation = {}; $s.Save()",
            q(link.as_path()),
            q(exe.as_path()),
            q(dir),
            q(exe.as_path())
        );
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("could not run PowerShell: {e}"))?;
        if !status.success() || !link.exists() {
            return Err(format!("the shortcut could not be created ({status})"));
        }
        Ok(link.display().to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, link);
        Err("desktop shortcuts are made by the Windows installer or on Windows only".into())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorInfo {
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
    primary: bool,
}

#[tauri::command]
fn list_monitors(app: AppHandle) -> Vec<MonitorInfo> {
    let primary = main_monitor(&app).map(|m| monitor_rect(&m));
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let (x, y, width, height) = monitor_rect(m);
            MonitorInfo {
                name: m.name().cloned().unwrap_or_default(),
                x,
                y,
                width,
                height,
                scale: m.scale_factor(),
                primary: primary == Some((x, y, width, height)),
            }
        })
        .collect()
}

/// Where the minimap sits on monitor `index` with the default HUD.
#[tauri::command]
fn default_minimap_region(app: AppHandle, index: usize) -> Result<ScreenRegion, String> {
    let monitors = app.available_monitors().map_err(|e| e.to_string())?;
    let m = monitors.get(index).ok_or("no such monitor")?;
    let (x, y, w, h) = monitor_rect(m);
    Ok(minimap::default_region(x, y, w, h))
}

/// A screenshot of monitor `index` for picking the minimap:
/// `[width u32][height u32][x i32][y i32]` + RGBA. The settings window steps
/// aside for it.
#[tauri::command]
async fn screenshot_monitor(app: AppHandle, index: usize) -> Result<tauri::ipc::Response, String> {
    let monitors = app.available_monitors().map_err(|e| e.to_string())?;
    let (x, y, w, h) = monitor_rect(monitors.get(index).ok_or("no such monitor")?);
    let settings = app.get_webview_window(SETTINGS);
    if let Some(win) = &settings {
        let _ = win.hide();
    }
    std::thread::sleep(Duration::from_millis(350));
    let shot = radar_vision::capture::grab_once(radar_vision::ScreenRect {
        x,
        y,
        width: w,
        height: h,
    });
    if let Some(win) = &settings {
        let _ = win.show();
        let _ = win.set_focus();
    }
    let mut px = shot.map_err(|e| e.to_string())?;
    radar_vision::capture::bgrx_to_rgba(&mut px);
    let mut out = Vec::with_capacity(16 + px.len());
    out.extend(w.to_le_bytes());
    out.extend(h.to_le_bytes());
    out.extend(x.to_le_bytes());
    out.extend(y.to_le_bytes());
    out.extend(px);
    Ok(tauri::ipc::Response::new(out))
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

/// Ctrl+Alt+S opens the settings: the way back when the radar lets clicks
/// through. Global, so it works while the game has focus.
fn settings_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyS)
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
                    if *shortcut == settings_shortcut() {
                        // The handler runs on the event loop, where windows are
                        // made (as in setup), so the window can be built here.
                        if let Err(e) = show_settings_window(app) {
                            log::warn!("settings window: {e}");
                        }
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
                    minimap: started.minimap,
                }),
                store,
                paths,
            });
            if let Err(e) = app.global_shortcut().register(settings_shortcut()) {
                log::warn!("could not register the settings hotkey (Ctrl+Alt+S): {e}");
            }
            for (shortcut, kind) in marker_shortcuts() {
                // Another program may own the key; markers still work from the settings window.
                if let Err(e) = app.global_shortcut().register(shortcut) {
                    log::warn!("could not register the {kind} hotkey: {e}");
                }
            }
            create_radar_window(&handle, &settings)?;
            sync_guide_window(&handle, &settings);
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
                        let (engine, minimap) = {
                            let mut inner = state.lock();
                            (inner.engine.take(), inner.minimap.take())
                        };
                        if let Some(engine) = engine {
                            engine.stop();
                        }
                        if let Some(m) = minimap {
                            m.stop();
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
            get_recordings_dir,
            read_sound_file,
            get_minimap_status,
            get_minimap_view,
            minimap_frame,
            list_monitors,
            default_minimap_region,
            screenshot_monitor,
            create_desktop_shortcut
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
