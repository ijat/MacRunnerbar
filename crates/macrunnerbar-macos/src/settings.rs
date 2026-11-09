//! `SettingsStore` (a JSON file) and `Clock` (the system clock).

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use macrunnerbar_core::{Clock, OsError, Settings, SettingsStore, Timestamp};

use crate::paths::Paths;

/// Preferences in `settings.json` under Application Support. Never holds the token.
#[derive(Debug, Clone)]
pub struct JsonSettingsStore {
    path: PathBuf,
}

impl JsonSettingsStore {
    /// The store for `paths`.
    pub fn new(paths: &Paths) -> Self {
        Self {
            path: paths.settings_file(),
        }
    }
}

impl SettingsStore for JsonSettingsStore {
    fn load(&self) -> Result<Settings, OsError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| OsError::new("settings.json is not valid")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(OsError::new(format!(
                "cannot read settings: {:?}",
                e.kind()
            ))),
        }
    }

    fn save(&self, settings: &Settings) -> Result<(), OsError> {
        let io = |e: std::io::Error| OsError::new(format!("cannot write settings: {:?}", e.kind()));
        if let Some(dir) = self.path.parent() {
            crate::paths::create_private_dir(dir)?;
        }
        let text = serde_json::to_vec_pretty(settings)
            .map_err(|_| OsError::new("cannot encode settings"))?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text)
            .and_then(|()| fs::rename(&tmp, &self.path))
            .map_err(io)
    }
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        Timestamp::from_millis(ms)
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}
