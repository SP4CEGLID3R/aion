//! Tauri shell: windows, settings persistence, and the bridge between the
//! ingest pipeline and the webviews. All radar logic lives in `radar-core`
//! and `radar-ingest`; this crate only wires them to the desktop.

mod store;

use std::sync::{Arc, Mutex, MutexGuard};

use radar_core::{RadarFrame, RadarSettings, WindowMode};
use radar_ingest::{DebugReport, Engine, FrameSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use store::SettingsStore;

const RADAR: &str = "radar";
const SETTINGS: &str = "settings";
const FRAME_EVENT: &str = "radar-frame";
const SETTINGS_EVENT: &str = "settings-changed";

struct Inner {
    settings: RadarSettings,
    engine: Option<Engine>,
}

struct AppState {
    inner: Mutex<Inner>,
    store: SettingsStore,
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

fn start_engine(app: &AppHandle, settings: &RadarSettings) -> (Option<Engine>, Vec<String>) {
    match Engine::start(settings, Arc::new(TauriSink(app.clone()))) {
        Ok(engine) => (Some(engine), Vec::new()),
        Err(e) => {
            log::error!("could not start the radar pipeline: {e}");
            (None, vec![format!("radar pipeline did not start: {e}")])
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
            let (engine, errors) = start_engine(&app, &settings);
            inner.engine = engine;
            warnings.extend(errors);
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

pub fn run() {
    tauri::Builder::default()
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

            let (engine, _) = start_engine(&handle, &loaded.settings);
            let settings = loaded.settings.clone();
            // State first: the windows' scripts call commands as soon as they load.
            app.manage(AppState {
                inner: Mutex::new(Inner {
                    settings: loaded.settings,
                    engine,
                }),
                store,
            });
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
            open_settings_window
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
