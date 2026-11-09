//! The composition root: builds the real adapters, the engine and the services the UI calls.
//! This is the only place that names concrete adapters; everything else uses the core traits.

use std::fs::File;
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use macrunnerbar_core::engine::{Deps, Engine, EngineConfig, EngineHandle, Observer};
use macrunnerbar_core::{
    ApiError, GitHubApi, JitConfig, JobInfo, Keychain, LoginItem, OsError, QueuedJob, Repo,
    RunnerInfo, RunnerRelease, Secret, Settings, SettingsStore,
};
use macrunnerbar_github::GitHubClient;
use macrunnerbar_macos::logging::Redactor;
use macrunnerbar_macos::{
    JsonSettingsStore, LaunchAgent, OsascriptNotifier, Paths, ProcessRunnerHost,
    SecurityCliKeychain, SystemClock, logging,
};

/// A `GitHubApi` whose token can change while the app runs (Preferences). With no token every call is
/// `Unauthorized`, which the engine treats as an account-wide problem and backs off from.
#[derive(Default)]
pub(crate) struct GitHubSlot {
    client: RwLock<Option<Arc<GitHubClient>>>,
}

impl std::fmt::Debug for GitHubSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubSlot")
            .field("has_token", &self.has_token())
            .finish()
    }
}

impl GitHubSlot {
    /// Use `token` from now on.
    pub(crate) fn set_token(&self, token: Secret<String>) {
        *self.client.write().unwrap_or_else(PoisonError::into_inner) =
            Some(Arc::new(GitHubClient::new(token)));
    }

    /// Forget the token.
    pub(crate) fn clear(&self) {
        *self.client.write().unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub(crate) fn has_token(&self) -> bool {
        self.client
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn client(&self) -> Result<Arc<GitHubClient>, ApiError> {
        self.client
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or(ApiError::Unauthorized)
    }
}

impl GitHubApi for GitHubSlot {
    fn list_repos(&self) -> Result<Vec<Repo>, ApiError> {
        self.client()?.list_repos()
    }

    fn queued_jobs(&self, repo: &str) -> Result<Vec<QueuedJob>, ApiError> {
        self.client()?.queued_jobs(repo)
    }

    fn generate_jit(
        &self,
        repo: &str,
        runner_name: &str,
        labels: &[String],
    ) -> Result<JitConfig, ApiError> {
        self.client()?.generate_jit(repo, runner_name, labels)
    }

    fn list_runners(&self, repo: &str) -> Result<Vec<RunnerInfo>, ApiError> {
        self.client()?.list_runners(repo)
    }

    fn delete_runner(&self, repo: &str, runner_id: u64) -> Result<(), ApiError> {
        self.client()?.delete_runner(repo, runner_id)
    }

    fn latest_runner_release(&self) -> Result<RunnerRelease, ApiError> {
        self.client()?.latest_runner_release()
    }

    fn download_release(&self, release: &RunnerRelease, dest: &Path) -> Result<(), ApiError> {
        self.client()?.download_release(release, dest)
    }

    fn find_job_by_runner(
        &self,
        repo: &str,
        runner_name: &str,
    ) -> Result<Option<JobInfo>, ApiError> {
        self.client()?.find_job_by_runner(repo, runner_name)
    }

    fn get_job(&self, repo: &str, job_id: u64) -> Result<JobInfo, ApiError> {
        self.client()?.get_job(repo, job_id)
    }
}

/// What Preferences needs besides the engine.
pub(crate) struct Services {
    pub(crate) keychain: SecurityCliKeychain,
    pub(crate) settings: Arc<JsonSettingsStore>,
    pub(crate) login: LaunchAgent,
    pub(crate) github: Arc<GitHubSlot>,
    pub(crate) redactor: Redactor,
    /// Held for the life of the process: a second instance would fight this one over `ghm-` runners.
    _lock: File,
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services").finish_non_exhaustive()
    }
}

/// Why start-up stopped.
#[derive(Debug)]
pub(crate) enum StartError {
    /// Another instance holds the lock.
    AlreadyRunning,
    Failed(String),
}

fn failed(what: &str, e: &OsError) -> StartError {
    StartError::Failed(format!("{what}: {e}"))
}

/// Takes the single-instance lock (a `flock` on a file under Application Support).
fn single_instance(paths: &Paths) -> Result<File, StartError> {
    paths.ensure().map_err(|e| failed("app directories", &e))?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.root().join("instance.lock"))
        .map_err(|e| StartError::Failed(format!("lock file: {:?}", e.kind())))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(StartError::AlreadyRunning),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(StartError::Failed(format!("lock file: {:?}", e.kind())))
        }
    }
}

impl Services {
    /// Builds the adapters and starts the engine (which cleans up leftovers first).
    pub(crate) fn start(observer: Observer) -> Result<(Self, EngineHandle), StartError> {
        let paths = Paths::from_env().map_err(|e| failed("home directory", &e))?;
        let lock = single_instance(&paths)?;
        let redactor = logging::init(paths.logs_dir(), tracing::Level::INFO)
            .map_err(|e| failed("logging", &e))?;
        let keychain = SecurityCliKeychain::standard();
        let github = Arc::new(GitHubSlot::default());
        match keychain.get_token() {
            Ok(Some(token)) => {
                redactor.add(&token);
                github.set_token(token);
            }
            Ok(None) => tracing::info!("no token in the Keychain yet"),
            Err(e) => tracing::warn!(error = %e, "cannot read the token from the Keychain"),
        }
        let settings = Arc::new(JsonSettingsStore::new(&paths));
        let exe = std::env::current_exe()
            .map_err(|e| StartError::Failed(format!("executable path: {:?}", e.kind())))?;
        let login = LaunchAgent::new(paths.launch_agents_dir(), &exe)
            .map_err(|e| failed("login item", &e))?;
        let deps = Deps {
            github: github.clone(),
            host: Arc::new(ProcessRunnerHost::new(&paths)),
            clock: Arc::new(SystemClock),
            notifier: Arc::new(OsascriptNotifier::default()),
            settings: settings.clone(),
        };
        let engine = Engine::new(deps, EngineConfig::new(paths.root().join("downloads")));
        let handle = EngineHandle::spawn(engine, Some(observer));
        Ok((
            Self {
                keychain,
                settings,
                login,
                github,
                redactor,
                _lock: lock,
            },
            handle,
        ))
    }

    /// The saved settings (defaults if there are none or they cannot be read).
    pub(crate) fn load_settings(&self) -> Settings {
        self.settings.load().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "cannot read settings; using defaults");
            Settings::default()
        })
    }

    /// Whether the Keychain holds a token.
    pub(crate) fn token_stored(&self) -> bool {
        matches!(self.keychain.get_token(), Ok(Some(_)))
    }

    /// Whether the login item is installed.
    pub(crate) fn login_enabled(&self) -> bool {
        self.login.is_enabled().unwrap_or(false)
    }

    /// Stores the token in the Keychain and starts using it. The value goes straight to the Keychain
    /// trait and the redactor; it is never logged.
    pub(crate) fn save_token(&self, token: Secret<String>) -> Result<(), String> {
        self.keychain
            .set_token(&token)
            .map_err(|e| format!("Keychain: {e}"))?;
        self.redactor.add(&token);
        self.github.set_token(token);
        Ok(())
    }

    /// Removes the stored token and stops using it.
    pub(crate) fn remove_token(&self) -> Result<(), String> {
        self.keychain
            .delete_token()
            .map_err(|e| format!("Keychain: {e}"))?;
        self.github.clear();
        Ok(())
    }

    /// Persists the settings and applies start at login.
    pub(crate) fn save_settings(&self, s: &Settings) -> Result<(), String> {
        self.settings
            .save(s)
            .map_err(|e| format!("Settings: {e}"))?;
        self.login
            .set_enabled(s.launch_at_login)
            .map_err(|e| format!("Login item: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_no_token_every_call_is_unauthorized() {
        let slot = GitHubSlot::default();
        assert!(!slot.has_token());
        assert_eq!(
            slot.list_repos().expect_err("no token"),
            ApiError::Unauthorized
        );
        assert_eq!(
            slot.queued_jobs("o/r").expect_err("no token"),
            ApiError::Unauthorized
        );
        assert_eq!(
            slot.get_job("o/r", 1).expect_err("no token"),
            ApiError::Unauthorized
        );
        assert_eq!(
            slot.latest_runner_release().expect_err("no token"),
            ApiError::Unauthorized
        );
    }

    #[test]
    fn a_token_can_be_set_and_cleared_while_running_and_never_prints() {
        let slot = GitHubSlot::default();
        let sentinel = concat!("gh", "p_SLOTSENTINEL0123456789abcdef");
        slot.set_token(Secret::new(sentinel.into()));
        assert!(slot.has_token());
        assert!(!format!("{slot:?}").contains(sentinel));
        slot.clear();
        assert!(!slot.has_token());
        assert_eq!(
            slot.list_repos().expect_err("cleared"),
            ApiError::Unauthorized
        );
    }

    #[test]
    fn the_second_instance_is_refused_and_the_lock_frees_on_drop() {
        let base = std::env::temp_dir().join(format!("mrb-app-lock-{}", std::process::id()));
        let paths = Paths::under(&base);
        let first = single_instance(&paths).expect("first instance");
        assert!(matches!(
            single_instance(&paths),
            Err(StartError::AlreadyRunning)
        ));
        drop(first);
        let again = single_instance(&paths).expect("lock released");
        drop(again);
        let _ = std::fs::remove_dir_all(&base);
    }
}
