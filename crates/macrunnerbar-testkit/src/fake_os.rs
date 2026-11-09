//! In-memory Keychain, settings store, clock, notifier and login item.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use macrunnerbar_core::{
    Clock, Keychain, LoginItem, Notification, Notifier, OsError, Secret, Settings, SettingsStore,
    Timestamp,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Fake [`Keychain`].
#[derive(Debug, Default)]
pub struct FakeKeychain {
    token: Mutex<Option<Secret<String>>>,
}

impl FakeKeychain {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Holding `token`.
    pub fn with_token(token: &str) -> Self {
        Self {
            token: Mutex::new(Some(Secret::new(token.into()))),
        }
    }
}

impl Keychain for FakeKeychain {
    fn get_token(&self) -> Result<Option<Secret<String>>, OsError> {
        Ok(lock(&self.token).clone())
    }

    fn set_token(&self, token: &Secret<String>) -> Result<(), OsError> {
        *lock(&self.token) = Some(token.clone());
        Ok(())
    }

    fn delete_token(&self) -> Result<(), OsError> {
        *lock(&self.token) = None;
        Ok(())
    }
}

/// Fake [`SettingsStore`].
#[derive(Debug, Default)]
pub struct MemSettingsStore {
    stored: Mutex<Option<Settings>>,
}

impl MemSettingsStore {
    /// Nothing stored (loads the defaults).
    pub fn new() -> Self {
        Self::default()
    }
}

impl SettingsStore for MemSettingsStore {
    fn load(&self) -> Result<Settings, OsError> {
        Ok(lock(&self.stored).clone().unwrap_or_default())
    }

    fn save(&self, settings: &Settings) -> Result<(), OsError> {
        *lock(&self.stored) = Some(settings.clone());
        Ok(())
    }
}

/// Fake [`Clock`]: `sleep` advances time instead of blocking.
#[derive(Debug)]
pub struct FakeClock {
    now_ms: AtomicU64,
}

impl FakeClock {
    /// Starting at `start`.
    pub fn new(start: Timestamp) -> Self {
        Self {
            now_ms: AtomicU64::new(start.as_millis()),
        }
    }

    /// Move time forward without sleeping.
    pub fn advance(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.now_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new(Timestamp::from_millis(1_700_000_000_000))
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.now_ms.load(Ordering::SeqCst))
    }

    fn sleep(&self, d: Duration) {
        self.advance(d);
    }
}

/// Fake [`Notifier`] that records what it was asked to show.
#[derive(Debug, Default)]
pub struct RecordingNotifier {
    sent: Mutex<Vec<Notification>>,
}

impl RecordingNotifier {
    /// Nothing sent yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Notifications so far, oldest first.
    pub fn sent(&self) -> Vec<Notification> {
        lock(&self.sent).clone()
    }
}

impl Notifier for RecordingNotifier {
    fn notify(&self, n: &Notification) -> Result<(), OsError> {
        lock(&self.sent).push(n.clone());
        Ok(())
    }
}

/// Fake [`LoginItem`].
#[derive(Debug, Default)]
pub struct FakeLoginItem {
    enabled: AtomicBool,
}

impl FakeLoginItem {
    /// Disabled.
    pub fn new() -> Self {
        Self::default()
    }
}

impl LoginItem for FakeLoginItem {
    fn is_enabled(&self) -> Result<bool, OsError> {
        Ok(self.enabled.load(Ordering::SeqCst))
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), OsError> {
        self.enabled.store(enabled, Ordering::SeqCst);
        Ok(())
    }
}
