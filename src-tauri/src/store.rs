//! Settings persistence: one JSON file in the app's config directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use radar_core::RadarSettings;

pub struct SettingsStore {
    path: PathBuf,
}

pub struct Loaded {
    pub settings: RadarSettings,
    pub warnings: Vec<String>,
    /// No settings file existed yet.
    pub first_run: bool,
}

impl SettingsStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("settings.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads and sanitizes settings. A corrupt file is moved aside (so the
    /// user's values are not lost) and defaults are used.
    pub fn load(&self) -> Loaded {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Loaded {
                    settings: RadarSettings::default(),
                    warnings: Vec::new(),
                    first_run: true,
                };
            }
            Err(e) => {
                return Loaded {
                    settings: RadarSettings::default(),
                    warnings: vec![format!(
                        "could not read {}: {e}; using defaults",
                        self.path.display()
                    )],
                    first_run: false,
                };
            }
        };
        match serde_json::from_str::<RadarSettings>(&text) {
            Ok(s) => {
                let (settings, warnings) = s.sanitized();
                Loaded {
                    settings,
                    warnings,
                    first_run: false,
                }
            }
            Err(e) => {
                let backup = self.path.with_extension("json.bak");
                let moved = fs::rename(&self.path, &backup).is_ok();
                let mut warnings = vec![format!("settings file is invalid ({e}); using defaults")];
                if moved {
                    warnings.push(format!("the old file was kept as {}", backup.display()));
                }
                Loaded {
                    settings: RadarSettings::default(),
                    warnings,
                    first_run: false,
                }
            }
        }
    }

    /// Writes atomically: a temporary file, then a rename over the old one.
    pub fn save(&self, settings: &RadarSettings) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aion2-radar-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_is_a_first_run_with_defaults() {
        let dir = temp_dir("missing");
        let loaded = SettingsStore::new(&dir).load();
        assert!(loaded.first_run);
        assert_eq!(loaded.settings, RadarSettings::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let store = SettingsStore::new(&dir);
        let s = RadarSettings {
            range: 321.0,
            ..Default::default()
        };
        store.save(&s).unwrap();
        let loaded = store.load();
        assert!(!loaded.first_run);
        assert_eq!(loaded.settings.range, 321.0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaults_used() {
        let dir = temp_dir("corrupt");
        let store = SettingsStore::new(&dir);
        fs::write(store.path(), "{ not json").unwrap();
        let loaded = store.load();
        assert_eq!(loaded.settings, RadarSettings::default());
        assert_eq!(loaded.warnings.len(), 2);
        assert!(dir.join("settings.json.bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_range_values_on_disk_are_sanitized() {
        let dir = temp_dir("sanitize");
        let store = SettingsStore::new(&dir);
        fs::write(store.path(), r#"{"opacity": 5}"#).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.settings.opacity, 1.0);
        assert_eq!(loaded.warnings.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
