//! In-memory GitHub.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use macrunnerbar_core::{
    ApiError, GitHubApi, JitConfig, QueuedJob, Repo, RunnerInfo, RunnerRelease, Secret,
};

#[derive(Debug, Default)]
struct State {
    repos: Vec<Repo>,
    queued: HashMap<String, Vec<QueuedJob>>,
    runners: HashMap<String, Vec<RunnerInfo>>,
    next_runner_id: u64,
    /// Persistent failure per repo (lowercase), returned by every repo-scoped call.
    repo_failures: HashMap<String, ApiError>,
    /// One-shot failures for the next `generate_jit` calls.
    jit_failures: VecDeque<ApiError>,
    /// Failure for `list_repos`.
    list_failure: Option<ApiError>,
    release: Option<RunnerRelease>,
    calls: Vec<String>,
}

/// Fake [`GitHubApi`]. Seed it with the `add_*` / `set_*` methods; inspect it with `calls`/`runners`.
#[derive(Debug)]
pub struct FakeGitHub {
    inner: Mutex<State>,
}

/// The fake release claims this SHA-256; `download_release` accepts only it.
const FAKE_ARCHIVE_BYTES: &[u8] = b"fake-runner-archive";
const FAKE_SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

impl Default for FakeGitHub {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeGitHub {
    /// Empty, with one release `2.999.0`.
    pub fn new() -> Self {
        let state = State {
            next_runner_id: 1000,
            release: Some(RunnerRelease {
                version: "2.999.0".into(),
                url: "https://example.invalid/actions-runner-osx-arm64-2.999.0.tar.gz".into(),
                sha256: FAKE_SHA.into(),
            }),
            ..State::default()
        };
        Self {
            inner: Mutex::new(state),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Add a repo; seeds empty queue and runner list.
    pub fn add_repo(&self, repo: Repo) {
        let mut s = self.lock();
        let key = repo.full_name.to_ascii_lowercase();
        s.queued.entry(key.clone()).or_default();
        s.runners.entry(key).or_default();
        s.repos.push(repo);
    }

    /// Replace the queued jobs of `repo`.
    pub fn set_queued(&self, repo: &str, jobs: Vec<QueuedJob>) {
        self.lock().queued.insert(repo.to_ascii_lowercase(), jobs);
    }

    /// Make every repo-scoped call on `repo` fail with `err` until [`clear_failures`](Self::clear_failures).
    pub fn fail_repo(&self, repo: &str, err: ApiError) {
        self.lock()
            .repo_failures
            .insert(repo.to_ascii_lowercase(), err);
    }

    /// Fail the next `generate_jit` call once.
    pub fn fail_next_jit(&self, err: ApiError) {
        self.lock().jit_failures.push_back(err);
    }

    /// Make `list_repos` fail until [`clear_failures`](Self::clear_failures).
    pub fn fail_list_repos(&self, err: ApiError) {
        self.lock().list_failure = Some(err);
    }

    /// Remove every injected failure.
    pub fn clear_failures(&self) {
        let mut s = self.lock();
        s.repo_failures.clear();
        s.jit_failures.clear();
        s.list_failure = None;
    }

    /// Register a runner directly (a stale or foreign registration); returns its id.
    pub fn add_runner(&self, repo: &str, name: &str, online: bool, busy: bool) -> u64 {
        let mut s = self.lock();
        s.next_runner_id += 1;
        let id = s.next_runner_id;
        s.runners
            .entry(repo.to_ascii_lowercase())
            .or_default()
            .push(RunnerInfo {
                id,
                name: name.into(),
                online,
                busy,
            });
        id
    }

    /// Mark a registered runner busy or idle.
    pub fn set_runner_busy(&self, repo: &str, name: &str, busy: bool) {
        let mut s = self.lock();
        if let Some(r) = s
            .runners
            .get_mut(&repo.to_ascii_lowercase())
            .and_then(|v| v.iter_mut().find(|r| r.name == name))
        {
            r.busy = busy;
            r.online = true;
        }
    }

    /// Current registrations of `repo`.
    pub fn runners(&self, repo: &str) -> Vec<RunnerInfo> {
        self.lock()
            .runners
            .get(&repo.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    /// Replace the latest release the fake reports.
    pub fn set_release(&self, release: RunnerRelease) {
        self.lock().release = Some(release);
    }

    /// Calls so far, as `"method repo"` strings, oldest first.
    pub fn calls(&self) -> Vec<String> {
        self.lock().calls.clone()
    }

    fn repo_call(&self, method: &str, repo: &str) -> Result<MutexGuard<'_, State>, ApiError> {
        let mut s = self.lock();
        s.calls.push(format!("{method} {repo}"));
        let key = repo.to_ascii_lowercase();
        if let Some(e) = s.repo_failures.get(&key) {
            return Err(e.clone());
        }
        if !s.runners.contains_key(&key) {
            return Err(ApiError::NotFound);
        }
        Ok(s)
    }
}

impl GitHubApi for FakeGitHub {
    fn list_repos(&self) -> Result<Vec<Repo>, ApiError> {
        let mut s = self.lock();
        s.calls.push("list_repos".into());
        match &s.list_failure {
            Some(e) => Err(e.clone()),
            None => Ok(s.repos.clone()),
        }
    }

    fn queued_jobs(&self, repo: &str) -> Result<Vec<QueuedJob>, ApiError> {
        let s = self.repo_call("queued_jobs", repo)?;
        Ok(s.queued
            .get(&repo.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default())
    }

    fn generate_jit(
        &self,
        repo: &str,
        runner_name: &str,
        _labels: &[String],
    ) -> Result<JitConfig, ApiError> {
        let mut s = self.repo_call("generate_jit", repo)?;
        if let Some(e) = s.jit_failures.pop_front() {
            return Err(e);
        }
        s.next_runner_id += 1;
        let id = s.next_runner_id;
        s.runners
            .entry(repo.to_ascii_lowercase())
            .or_default()
            .push(RunnerInfo {
                id,
                name: runner_name.into(),
                online: false,
                busy: false,
            });
        Ok(JitConfig {
            runner_id: id,
            encoded: Secret::new(format!("jit-for-{runner_name}")),
        })
    }

    fn list_runners(&self, repo: &str) -> Result<Vec<RunnerInfo>, ApiError> {
        let s = self.repo_call("list_runners", repo)?;
        Ok(s.runners
            .get(&repo.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default())
    }

    fn delete_runner(&self, repo: &str, runner_id: u64) -> Result<(), ApiError> {
        let mut s = self.repo_call("delete_runner", repo)?;
        if let Some(v) = s.runners.get_mut(&repo.to_ascii_lowercase()) {
            v.retain(|r| r.id != runner_id);
        }
        Ok(())
    }

    fn latest_runner_release(&self) -> Result<RunnerRelease, ApiError> {
        let mut s = self.lock();
        s.calls.push("latest_runner_release".into());
        s.release.clone().ok_or(ApiError::NotFound)
    }

    fn download_release(&self, release: &RunnerRelease, dest: &Path) -> Result<(), ApiError> {
        self.lock().calls.push("download_release".into());
        if release.sha256 != FAKE_SHA {
            return Err(ApiError::Invalid("checksum mismatch".into()));
        }
        std::fs::write(dest, FAKE_ARCHIVE_BYTES).map_err(|e| ApiError::Network(e.to_string()))
    }
}
