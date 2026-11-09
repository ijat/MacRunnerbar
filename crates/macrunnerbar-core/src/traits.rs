//! Traits for everything external or replaceable. Implementations live in adapter crates; each has
//! a fake and a contract suite in `macrunnerbar-testkit`.

use std::path::Path;
use std::time::Duration;

use crate::error::{ApiError, OsError};
use crate::secret::Secret;
use crate::types::{
    JitConfig, Notification, ProcessState, QueuedJob, Repo, RunnerHandle, RunnerInfo,
    RunnerRelease, Settings, Timestamp,
};

/// The GitHub REST API, as far as this app uses it. Blocking.
pub trait GitHubApi: Send + Sync {
    /// Every repo the token can see (all pages).
    fn list_repos(&self) -> Result<Vec<Repo>, ApiError>;

    /// Jobs currently `queued` in `repo`, with their labels. `NotFound` if the repo is unknown.
    fn queued_jobs(&self, repo: &str) -> Result<Vec<QueuedJob>, ApiError>;

    /// Register a single-use JIT runner named `runner_name` in `repo`, carrying the configured labels.
    fn generate_jit(
        &self,
        repo: &str,
        runner_name: &str,
        labels: &[String],
    ) -> Result<JitConfig, ApiError>;

    /// Runners registered in `repo` (all pages).
    fn list_runners(&self, repo: &str) -> Result<Vec<RunnerInfo>, ApiError>;

    /// Delete a registration. Idempotent: an id that is already gone is `Ok(())`.
    fn delete_runner(&self, repo: &str, runner_id: u64) -> Result<(), ApiError>;

    /// The latest `actions/runner` osx-arm64 release and its expected SHA-256.
    fn latest_runner_release(&self) -> Result<RunnerRelease, ApiError>;

    /// Download `release` to `dest`, verifying its SHA-256. On a mismatch, `dest` is not left behind
    /// and the error is `ApiError::Invalid`.
    fn download_release(&self, release: &RunnerRelease, dest: &Path) -> Result<(), ApiError>;
}

/// Runner processes and their directories on this machine.
pub trait RunnerHost: Send + Sync {
    /// Version of the installed runner template, `None` if there is none yet.
    fn template_version(&self) -> Result<Option<String>, OsError>;

    /// Replace the template with the contents of the release archive at `archive`.
    fn install_template(&self, archive: &Path, version: &str) -> Result<(), OsError>;

    /// Copy the template to a fresh directory for `name` and start `run.sh --jitconfig` there.
    fn spawn(&self, name: &str, jit: &JitConfig) -> Result<RunnerHandle, OsError>;

    /// State of a process started by [`spawn`](Self::spawn).
    fn poll(&self, handle: RunnerHandle) -> Result<ProcessState, OsError>;

    /// Stop the process and its children. Idempotent.
    fn kill(&self, handle: RunnerHandle) -> Result<(), OsError>;

    /// Remove the directory of runner `name`, stopping anything still running from it. Idempotent.
    fn delete_dir(&self, name: &str) -> Result<(), OsError>;

    /// Names of runner directories left on disk (after a crash or quit).
    fn list_leftover(&self) -> Result<Vec<String>, OsError>;
}

/// Where the token is stored.
pub trait Keychain: Send + Sync {
    /// The stored token, `None` if absent.
    fn get_token(&self) -> Result<Option<Secret<String>>, OsError>;

    /// Store or replace the token.
    fn set_token(&self, token: &Secret<String>) -> Result<(), OsError>;

    /// Remove the token. Idempotent.
    fn delete_token(&self) -> Result<(), OsError>;
}

/// Where preferences are kept.
pub trait SettingsStore: Send + Sync {
    /// Stored settings, or the defaults if nothing is stored.
    fn load(&self) -> Result<Settings, OsError>;

    /// Persist settings.
    fn save(&self, settings: &Settings) -> Result<(), OsError>;
}

/// Wall clock and sleeping, so time can be faked.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> Timestamp;

    /// Block for `d`.
    fn sleep(&self, d: Duration);
}

/// Posts user-facing notifications.
pub trait Notifier: Send + Sync {
    /// Show `n`.
    fn notify(&self, n: &Notification) -> Result<(), OsError>;
}

/// Start-at-login registration.
pub trait LoginItem: Send + Sync {
    /// Whether the app starts at login.
    fn is_enabled(&self) -> Result<bool, OsError>;

    /// Turn start-at-login on or off. Idempotent.
    fn set_enabled(&self, enabled: bool) -> Result<(), OsError>;
}
