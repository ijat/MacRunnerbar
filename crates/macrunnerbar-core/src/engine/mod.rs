//! The engine: poll GitHub, start one JIT runner per queued job under the caps, reap finished
//! runners, keep the template fresh, and publish a [`StatusSnapshot`].
//!
//! [`Engine`] is a plain state machine driven by [`Engine::tick`]; it never sleeps and takes its time
//! from the [`Clock`] trait, so tests drive it with a fake clock. [`EngineHandle`] runs it on a worker
//! thread. Everything external (GitHub, processes, notifications, settings) is behind a core trait.
//!
//! Cadence: [`Engine::tick`] is cheap and may be called every couple of seconds. It polls runner
//! processes and retries cleanups every call, and does the GitHub scan only when the poll interval has
//! elapsed (or earlier: after a runner exits while jobs wait on a cap, after a wake from sleep, or on
//! request).

mod handle;
mod naming;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use handle::{EngineHandle, Observer};
pub use naming::{RUNNER_PREFIX, random_suffix, runner_name, slug};

use crate::policy::{
    BackoffPolicy, Load, RepoBackoff, RepoDemand, eligible_repos, matches, plan_spawns,
};
use crate::traits::{Clock, GitHubApi, Notifier, RunnerHost, SettingsStore};
use crate::types::{
    Notification, NotifyMode, ProcessState, QueuedJob, RunnerHandle, RunnerInfo, RunningJob,
    Settings, StatusSnapshot, Timestamp, WaitingJobs,
};
use crate::{ApiError, Repo};

/// Key under which account-wide failures (network, token, rate limit) are tracked.
const GLOBAL_KEY: &str = "*";

/// Collaborators the engine drives.
#[derive(Clone)]
pub struct Deps {
    /// GitHub.
    pub github: Arc<dyn GitHubApi>,
    /// Runner processes and directories.
    pub host: Arc<dyn RunnerHost>,
    /// Time.
    pub clock: Arc<dyn Clock>,
    /// Notifications.
    pub notifier: Arc<dyn Notifier>,
    /// Preferences (re-read at every scan).
    pub settings: Arc<dyn SettingsStore>,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps").finish_non_exhaustive()
    }
}

/// Tunables that are not user preferences.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Where release archives are downloaded to before installing (created if missing).
    pub scratch_dir: PathBuf,
    /// How often the eligible-repo list is refreshed.
    pub repo_refresh: Duration,
    /// An unclaimed runner older than this is stopped when no queued job needs it.
    pub idle_timeout: Duration,
    /// How often to look for a newer runner release.
    pub template_check_every: Duration,
    /// Retry delay after a failed template update.
    pub template_retry: Duration,
    /// A gap between ticks longer than this is treated as a sleep/wake.
    pub wake_gap: Duration,
    /// Backoff for failing repos and for account-wide failures.
    pub backoff: BackoffPolicy,
    /// Consecutive runner crashes in one repo that put the repo in backoff.
    pub crash_streak_limit: u32,
    /// Lower bound on the poll interval, whatever the settings say.
    pub min_poll: Duration,
}

impl EngineConfig {
    /// Defaults matching the Linux autoscaler: 10 min repo refresh and idle timeout, daily template check.
    pub fn new(scratch_dir: impl Into<PathBuf>) -> Self {
        Self {
            scratch_dir: scratch_dir.into(),
            repo_refresh: Duration::from_secs(600),
            idle_timeout: Duration::from_secs(600),
            template_check_every: Duration::from_secs(24 * 3600),
            template_retry: Duration::from_secs(3600),
            wake_gap: Duration::from_secs(120),
            backoff: BackoffPolicy::default(),
            crash_streak_limit: 3,
            min_poll: Duration::from_secs(5),
        }
    }
}

/// The job a runner was started for (best effort: a runner takes whichever queued job GitHub offers).
#[derive(Debug, Clone)]
struct JobHint {
    job_id: u64,
    workflow: Option<String>,
    name: String,
    html_url: String,
}

#[derive(Debug)]
struct Tracked {
    handle: RunnerHandle,
    name: String,
    repo: String,
    runner_id: u64,
    started: Timestamp,
    /// GitHub reported the runner busy at least once.
    claimed: bool,
    hint: Option<JobHint>,
}

/// A finished or abandoned runner whose directory and registration still have to go.
#[derive(Debug)]
struct Cleanup {
    name: String,
    repo: String,
    runner_id: Option<u64>,
    dir_done: bool,
    reg_done: bool,
}

/// The state machine. See the module docs.
pub struct Engine {
    deps: Deps,
    config: EngineConfig,
    settings: Settings,
    paused: bool,
    tracked: Vec<Tracked>,
    cleanups: Vec<Cleanup>,
    /// Eligible repos, refreshed every `repo_refresh`.
    repos: Vec<String>,
    next_repo_refresh: Timestamp,
    next_scan: Timestamp,
    next_template_check: Timestamp,
    template_retry_at: Timestamp,
    last_tick: Option<Timestamp>,
    backoff: RepoBackoff,
    global_until: Option<Timestamp>,
    /// Repos whose registrations were swept for strays at least once in this run.
    swept: HashSet<String>,
    /// Consecutive runners that crashed (non-zero exit) per repo, lowercase.
    crash_streak: HashMap<String, u32>,
    waiting: Vec<WaitingJobs>,
    /// What to show as the last error, by source (`*`, `template`, `settings`, repo name, ...).
    errors: BTreeMap<String, String>,
    scans: u64,
    name_counter: u64,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("paused", &self.paused)
            .field("tracked", &self.tracked.len())
            .finish_non_exhaustive()
    }
}

fn lc(s: &str) -> String {
    s.to_ascii_lowercase()
}

impl Engine {
    /// A new engine. Loads the settings once (defaults if that fails).
    pub fn new(deps: Deps, config: EngineConfig) -> Self {
        let now = deps.clock.now();
        let mut errors = BTreeMap::new();
        let settings = match deps.settings.load() {
            Ok(s) => s,
            Err(e) => {
                errors.insert("settings".to_string(), format!("settings: {e}"));
                Settings::default()
            }
        };
        let backoff = RepoBackoff::new(config.backoff);
        Self {
            deps,
            config,
            settings,
            paused: false,
            tracked: Vec::new(),
            cleanups: Vec::new(),
            repos: Vec::new(),
            next_repo_refresh: now,
            next_scan: now,
            next_template_check: now,
            template_retry_at: now,
            last_tick: None,
            backoff,
            global_until: None,
            swept: HashSet::new(),
            crash_streak: HashMap::new(),
            waiting: Vec::new(),
            errors,
            scans: 0,
            name_counter: 0,
        }
    }

    /// Remove what a previous run (crash, kill) left behind: runner directories, and the processes
    /// recorded for them. Registrations are swept per repo on its first visit.
    pub fn startup(&mut self) {
        match self.deps.host.list_leftover() {
            Ok(names) => {
                for name in names {
                    if !name.starts_with(RUNNER_PREFIX) {
                        continue;
                    }
                    tracing::info!(runner = %name, "removing leftover runner directory");
                    if let Err(e) = self.deps.host.delete_dir(&name) {
                        self.errors
                            .insert("startup".into(), format!("cleanup of {name}: {e}"));
                    }
                }
            }
            Err(e) => {
                self.errors
                    .insert("startup".into(), format!("cleanup: {e}"));
            }
        }
    }

    /// Pause or resume. Paused: nothing new is started, running runners are still tracked and reaped.
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused != paused {
            self.paused = paused;
            self.next_scan = self.deps.clock.now();
        }
    }

    /// Whether paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Scan on the next tick instead of waiting for the poll interval (also re-reads the settings).
    pub fn request_scan(&mut self) {
        self.next_scan = self.deps.clock.now();
        self.next_repo_refresh = self.next_scan;
    }

    /// One step. Never panics on adapter errors; they become backoff and `last_error`.
    pub fn tick(&mut self) {
        let now = self.deps.clock.now();
        let woke = self
            .last_tick
            .is_some_and(|t| now.since(t) > self.config.wake_gap);
        self.last_tick = Some(now);
        if woke {
            tracing::info!("long gap since the last tick (sleep?); rescanning now");
            self.backoff = RepoBackoff::new(self.config.backoff);
            self.global_until = None;
            self.errors.remove(GLOBAL_KEY);
            self.next_scan = now;
            self.next_repo_refresh = now;
        }
        let net_ok = self.global_until.is_none_or(|t| now >= t);
        self.run_cleanups(net_ok);
        self.reap_exited(now);
        if net_ok {
            self.maintain_template(now);
        }
        if now >= self.next_scan && net_ok {
            self.scan(now);
        }
    }

    /// Stop every runner, remove every job directory and registration (best effort), clear the state.
    pub fn shutdown(&mut self) {
        for t in std::mem::take(&mut self.tracked) {
            if let Err(e) = self.deps.host.kill(t.handle) {
                tracing::warn!(runner = %t.name, error = %e, "kill failed during shutdown");
            }
            self.cleanups.push(Cleanup {
                name: t.name,
                repo: t.repo,
                runner_id: Some(t.runner_id),
                dir_done: false,
                reg_done: false,
            });
        }
        self.run_cleanups(true);
        if let Ok(left) = self.deps.host.list_leftover() {
            for name in left.into_iter().filter(|n| n.starts_with(RUNNER_PREFIX)) {
                let _ = self.deps.host.delete_dir(&name);
            }
        }
        self.waiting.clear();
    }

    /// What to render.
    pub fn snapshot(&self) -> StatusSnapshot {
        let mut running: Vec<RunningJob> = self
            .tracked
            .iter()
            .map(|t| {
                let hint = t.hint.as_ref().filter(|_| t.claimed);
                RunningJob {
                    runner_name: t.name.clone(),
                    repo: t.repo.clone(),
                    workflow: hint.and_then(|h| h.workflow.clone()),
                    job_name: hint.map(|h| h.name.clone()),
                    html_url: hint.map(|h| h.html_url.clone()),
                    started: t.started,
                }
            })
            .collect();
        running.sort_by(|a, b| {
            a.started
                .cmp(&b.started)
                .then_with(|| a.runner_name.cmp(&b.runner_name))
        });
        StatusSnapshot {
            paused: self.paused,
            running,
            waiting: self.waiting.clone(),
            max_runners: self.settings.global_max,
            last_error: self.errors.values().next().cloned(),
            as_of: self.deps.clock.now(),
        }
    }

    // ---- cleanup and reaping -------------------------------------------------------------------

    fn run_cleanups(&mut self, net_ok: bool) {
        let host = self.deps.host.clone();
        let github = self.deps.github.clone();
        for c in &mut self.cleanups {
            if !c.dir_done {
                match host.delete_dir(&c.name) {
                    Ok(()) => c.dir_done = true,
                    Err(e) => {
                        tracing::warn!(runner = %c.name, error = %e, "cannot remove runner directory yet")
                    }
                }
            }
            if !c.reg_done {
                match c.runner_id {
                    Some(id) if c.name.starts_with(RUNNER_PREFIX) => {
                        if net_ok {
                            match github.delete_runner(&c.repo, id) {
                                Ok(()) => c.reg_done = true,
                                Err(e) => {
                                    tracing::warn!(runner = %c.name, error = %e, "cannot delete registration yet")
                                }
                            }
                        }
                    }
                    _ => c.reg_done = true,
                }
            }
        }
        self.cleanups.retain(|c| !(c.dir_done && c.reg_done));
    }

    fn reap_exited(&mut self, now: Timestamp) {
        let mut finished: Vec<(Tracked, ProcessState)> = Vec::new();
        let mut keep = Vec::new();
        for t in std::mem::take(&mut self.tracked) {
            match self.deps.host.poll(t.handle) {
                Ok(ProcessState::Running) => keep.push(t),
                Ok(state) => finished.push((t, state)),
                Err(e) => {
                    // The host lost track of it: treat as gone and clean up by name.
                    tracing::warn!(runner = %t.name, error = %e, "runner state unknown; treating as exited");
                    finished.push((t, ProcessState::Exited(None)));
                }
            }
        }
        self.tracked = keep;
        for (t, state) in finished {
            self.on_exit(t, state, now);
        }
    }

    fn on_exit(&mut self, t: Tracked, state: ProcessState, now: Timestamp) {
        let code = match state {
            ProcessState::Exited(code) => code,
            ProcessState::Running => None,
        };
        let crashed = code != Some(0);
        let key = lc(&t.repo);
        if crashed {
            let n = self.crash_streak.entry(key.clone()).or_default();
            *n += 1;
            let streak = *n;
            tracing::warn!(runner = %t.name, repo = %t.repo, ?code, streak, "runner exited abnormally");
            if streak >= self.config.crash_streak_limit {
                let delay = self.backoff.record_failure(&t.repo, now);
                self.crash_streak.insert(key, 0);
                self.errors.insert(
                    t.repo.clone(),
                    format!(
                        "{}: {} runners in a row failed; retrying in {}s",
                        t.repo,
                        streak,
                        delay.as_secs()
                    ),
                );
            }
        } else {
            self.crash_streak.remove(&key);
            self.errors.remove(&t.repo);
        }
        self.notify_exit(&t, code);
        self.cleanups.push(Cleanup {
            name: t.name,
            repo: t.repo,
            runner_id: Some(t.runner_id),
            dir_done: false,
            reg_done: false,
        });
        self.run_cleanups(self.global_until.is_none_or(|u| now >= u));
        if !self.waiting.is_empty() {
            self.next_scan = now; // a slot just freed: do not wait for the poll interval
        }
    }

    fn notify_exit(&self, t: &Tracked, code: Option<i32>) {
        let crashed = code != Some(0);
        let send = match self.settings.notify {
            NotifyMode::Off => false,
            NotifyMode::FailuresOnly => crashed,
            NotifyMode::All => true,
        };
        if !send {
            return;
        }
        let what = match code {
            Some(0) => "finished".to_string(),
            Some(c) => format!("exited with code {c}"),
            None => "was stopped".to_string(),
        };
        let n = Notification {
            title: if crashed {
                "Runner failed".into()
            } else {
                "Job finished".into()
            },
            body: format!("{}: runner {what}", t.repo),
        };
        if let Err(e) = self.deps.notifier.notify(&n) {
            tracing::warn!(error = %e, "notification failed");
        }
    }

    // ---- template --------------------------------------------------------------------------------

    fn template_ready(&self) -> bool {
        matches!(self.deps.host.template_version(), Ok(Some(_)))
    }

    fn maintain_template(&mut self, now: Timestamp) {
        let installed = match self.deps.host.template_version() {
            Ok(v) => v,
            Err(e) => {
                self.errors
                    .insert("template".into(), format!("runner template: {e}"));
                return;
            }
        };
        let due = installed.is_none() || now >= self.next_template_check;
        if !due || now < self.template_retry_at {
            return;
        }
        match self.update_template(installed.as_deref()) {
            Ok(()) => {
                self.next_template_check = now.plus(self.config.template_check_every);
                self.errors.remove("template");
            }
            Err(msg) => {
                tracing::warn!(error = %msg, "runner template update failed");
                self.template_retry_at = now.plus(self.config.template_retry);
                // A failed refresh of a working template is only logged; having none is an error.
                if installed.is_none() {
                    self.errors
                        .insert("template".into(), format!("runner template: {msg}"));
                }
            }
        }
    }

    fn update_template(&self, installed: Option<&str>) -> Result<(), String> {
        let release = self
            .deps
            .github
            .latest_runner_release()
            .map_err(|e| e.to_string())?;
        if installed == Some(release.version.as_str()) {
            return Ok(());
        }
        let safe: String = release
            .version
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
            .collect();
        std::fs::create_dir_all(&self.config.scratch_dir)
            .map_err(|e| format!("scratch dir: {:?}", e.kind()))?;
        let archive = self
            .config
            .scratch_dir
            .join(format!("actions-runner-osx-arm64-{safe}.tar.gz"));
        let result = self
            .deps
            .github
            .download_release(&release, &archive)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                self.deps
                    .host
                    .install_template(&archive, &release.version)
                    .map_err(|e| e.to_string())
            });
        let _ = std::fs::remove_file(&archive);
        result?;
        tracing::info!(version = %release.version, "runner template updated");
        Ok(())
    }

    // ---- scanning --------------------------------------------------------------------------------

    fn reload_settings(&mut self) {
        match self.deps.settings.load() {
            Ok(s) => {
                self.settings = s;
                self.errors.remove("settings");
            }
            Err(e) => {
                self.errors.insert(
                    "settings".into(),
                    format!("settings: {e}; using the previous ones"),
                );
            }
        }
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.settings.poll_interval_secs).max(self.config.min_poll)
    }

    fn refresh_repos(&mut self, now: Timestamp) -> Result<(), ApiError> {
        let repos: Vec<Repo> = self.deps.github.list_repos()?;
        self.repos = eligible_repos(&repos, &self.settings);
        self.next_repo_refresh = now.plus(self.config.repo_refresh);
        tracing::info!(count = self.repos.len(), "watching repos");
        Ok(())
    }

    fn scan(&mut self, now: Timestamp) {
        self.reload_settings();
        self.scans += 1;
        let serve = !self.paused;
        self.next_scan = now.plus(self.poll_interval());
        let mut global_failed = false;

        if serve
            && now >= self.next_repo_refresh
            && let Err(e) = self.refresh_repos(now)
        {
            self.on_error(GLOBAL_KEY, &e, now, "listing repos");
            global_failed = true;
            // Retry the list soon, not in 10 minutes.
            self.next_repo_refresh = now;
        }

        let mut visit: Vec<String> = Vec::new();
        if serve && !global_failed {
            visit.extend(self.repos.iter().cloned());
        }
        for t in &self.tracked {
            if !visit.iter().any(|r| r.eq_ignore_ascii_case(&t.repo)) {
                visit.push(t.repo.clone());
            }
        }
        // Rotate the start so the same repo does not always win a scarce slot.
        if !visit.is_empty() {
            let len = visit.len();
            visit.rotate_left(usize::try_from(self.scans).unwrap_or(0) % len);
        }

        let template_ok = self.template_ready();
        let mut demands: Vec<RepoDemand> = Vec::new();
        let mut queued_by_repo: HashMap<String, Vec<QueuedJob>> = HashMap::new();
        if !global_failed {
            for repo in visit {
                if self.backoff.is_backed_off(&repo, now) {
                    continue;
                }
                match self.scan_repo(&repo, serve, now) {
                    Ok(queued) => {
                        self.backoff.record_success(&repo);
                        self.errors.remove(&repo);
                        if serve && template_ok && !queued.is_empty() {
                            demands.push(RepoDemand {
                                repo: repo.clone(),
                                queued: queued.len(),
                                unclaimed: self.unclaimed(&repo),
                            });
                            queued_by_repo.insert(lc(&repo), queued);
                        }
                    }
                    Err(e) => {
                        let stop = e.is_global() || matches!(e, ApiError::RateLimited { .. });
                        self.on_error(&repo, &e, now, "polling");
                        if stop {
                            global_failed = true;
                            break;
                        }
                    }
                }
            }
        }
        if !global_failed {
            self.backoff.record_success(GLOBAL_KEY);
            self.errors.remove(GLOBAL_KEY);
            self.global_until = None;
        }

        if serve && !global_failed {
            if !template_ok {
                self.waiting.clear();
                return;
            }
            let load = Load::from_repos(self.tracked.iter().map(|t| t.repo.as_str()));
            let plan = plan_spawns(
                &demands,
                &load,
                self.settings.per_owner_max,
                self.settings.global_max,
            );
            self.waiting = plan.waiting;
            for repo in plan.spawn {
                self.start_runner(&repo, queued_by_repo.get(&lc(&repo)), now);
                if self.global_until.is_some_and(|u| now < u) {
                    break;
                }
            }
        } else if !serve {
            self.waiting.clear();
        }
    }

    fn unclaimed(&self, repo: &str) -> usize {
        self.tracked
            .iter()
            .filter(|t| t.repo.eq_ignore_ascii_case(repo) && !t.claimed)
            .count()
    }

    /// Poll one repo: queued jobs that fit our labels, runner states, stray registrations, idle runners.
    /// Returns the matching queued jobs (empty when paused).
    fn scan_repo(
        &mut self,
        repo: &str,
        serve: bool,
        now: Timestamp,
    ) -> Result<Vec<QueuedJob>, ApiError> {
        let queued: Vec<QueuedJob> = if serve {
            self.deps
                .github
                .queued_jobs(repo)?
                .into_iter()
                .filter(|j| matches(&j.labels, &self.settings.labels))
                .collect()
        } else {
            Vec::new()
        };
        let has_tracked = self
            .tracked
            .iter()
            .any(|t| t.repo.eq_ignore_ascii_case(repo));
        if has_tracked || !self.swept.contains(&lc(repo)) {
            let runners = self.deps.github.list_runners(repo)?;
            self.apply_runner_states(repo, &runners);
            self.sweep_strays(repo, &runners);
            self.swept.insert(lc(repo));
            self.reap_idle(repo, queued.len(), now);
        }
        Ok(queued)
    }

    fn apply_runner_states(&mut self, repo: &str, runners: &[RunnerInfo]) {
        for t in self
            .tracked
            .iter_mut()
            .filter(|t| t.repo.eq_ignore_ascii_case(repo))
        {
            if runners.iter().any(|r| r.name == t.name && r.busy) {
                t.claimed = true;
            }
        }
    }

    /// Delete `ghm-*` registrations that are offline, idle and not ours (left by a crash).
    fn sweep_strays(&mut self, repo: &str, runners: &[RunnerInfo]) {
        for r in runners {
            if !r.name.starts_with(RUNNER_PREFIX)
                || r.online
                || r.busy
                || self.tracked.iter().any(|t| t.name == r.name)
                || self.cleanups.iter().any(|c| c.name == r.name)
            {
                continue;
            }
            tracing::info!(repo, runner = %r.name, "deleting stray registration");
            self.cleanups.push(Cleanup {
                name: r.name.clone(),
                repo: repo.to_string(),
                runner_id: Some(r.id),
                dir_done: true,
                reg_done: false,
            });
        }
        self.run_cleanups(true);
    }

    /// Stop unclaimed runners that nothing needs: more of them than queued jobs, and older than the
    /// idle timeout. A claimed (busy) runner is never touched.
    fn reap_idle(&mut self, repo: &str, queued: usize, now: Timestamp) {
        let unclaimed: Vec<usize> = self
            .tracked
            .iter()
            .enumerate()
            .filter(|(_, t)| t.repo.eq_ignore_ascii_case(repo) && !t.claimed)
            .map(|(i, _)| i)
            .collect();
        if queued >= unclaimed.len() {
            return;
        }
        let surplus = unclaimed.len() - queued;
        let mut candidates: Vec<usize> = unclaimed
            .into_iter()
            .filter(|&i| now.since(self.tracked[i].started) > self.config.idle_timeout)
            .collect();
        candidates.sort_by_key(|&i| self.tracked[i].started);
        candidates.truncate(surplus);
        candidates.sort_unstable_by(|a, b| b.cmp(a)); // remove from the back
        for i in candidates {
            let t = self.tracked.remove(i);
            tracing::info!(runner = %t.name, repo = %t.repo, "stopping idle runner");
            if let Err(e) = self.deps.host.kill(t.handle) {
                tracing::warn!(runner = %t.name, error = %e, "kill failed");
            }
            self.cleanups.push(Cleanup {
                name: t.name,
                repo: t.repo,
                runner_id: Some(t.runner_id),
                dir_done: false,
                reg_done: false,
            });
        }
        self.run_cleanups(true);
    }

    fn start_runner(&mut self, repo: &str, queued: Option<&Vec<QueuedJob>>, now: Timestamp) {
        self.name_counter += 1;
        let name = runner_name(repo, random_suffix(self.name_counter));
        let jit = match self
            .deps
            .github
            .generate_jit(repo, &name, &self.settings.labels)
        {
            Ok(j) => j,
            Err(e) => {
                self.on_error(repo, &e, now, "registering a runner");
                return;
            }
        };
        match self.deps.host.spawn(&name, &jit) {
            Ok(handle) => {
                let hint = queued.and_then(|q| {
                    let taken: HashSet<u64> = self
                        .tracked
                        .iter()
                        .filter_map(|t| t.hint.as_ref().map(|h| h.job_id))
                        .collect();
                    q.iter()
                        .find(|j| !taken.contains(&j.job_id))
                        .map(|j| JobHint {
                            job_id: j.job_id,
                            workflow: j.workflow.clone(),
                            name: j.name.clone(),
                            html_url: j.html_url.clone(),
                        })
                });
                tracing::info!(repo, runner = %name, "started runner");
                self.tracked.push(Tracked {
                    handle,
                    name,
                    repo: repo.to_string(),
                    runner_id: jit.runner_id,
                    started: now,
                    claimed: false,
                    hint,
                });
            }
            Err(e) => {
                tracing::warn!(repo, runner = %name, error = %e, "cannot start runner");
                // Do not leave the registration behind.
                self.cleanups.push(Cleanup {
                    name: name.clone(),
                    repo: repo.to_string(),
                    runner_id: Some(jit.runner_id),
                    dir_done: false,
                    reg_done: false,
                });
                self.run_cleanups(true);
                let delay = self.backoff.record_failure(repo, now);
                self.errors.insert(
                    repo.to_string(),
                    format!(
                        "{repo}: cannot start a runner: {e}; retrying in {}s",
                        delay.as_secs()
                    ),
                );
            }
        }
    }

    /// Record a GitHub failure: account-wide ones pause all API work, the rest back off that repo.
    fn on_error(&mut self, key: &str, e: &ApiError, now: Timestamp, what: &str) {
        match e {
            ApiError::RateLimited { retry_after } => {
                let own = self.backoff.record_failure(GLOBAL_KEY, now);
                let wait = retry_after.unwrap_or(own).max(Duration::from_secs(1));
                self.global_until = Some(now.plus(wait));
                self.errors.insert(
                    GLOBAL_KEY.into(),
                    format!("GitHub rate limit; retrying in {}s", wait.as_secs()),
                );
            }
            ApiError::Unauthorized | ApiError::Network(_) => {
                let wait = self.backoff.record_failure(GLOBAL_KEY, now);
                self.global_until = Some(now.plus(wait));
                self.errors.insert(
                    GLOBAL_KEY.into(),
                    format!("{e}; retrying in {}s", wait.as_secs()),
                );
            }
            ApiError::NotFound | ApiError::Forbidden(_) => {
                // Usually Actions or repo-level runners are disabled for this repo: skip it for a while
                // and keep it out of the error display.
                let wait = self.backoff.record_failure(key, now);
                tracing::info!(repo = key, error = %e, wait_secs = wait.as_secs(), "repo skipped");
            }
            other => {
                let wait = self.backoff.record_failure(key, now);
                self.errors.insert(
                    key.to_string(),
                    format!(
                        "{key}: {what} failed: {other}; retrying in {}s",
                        wait.as_secs()
                    ),
                );
            }
        }
    }
}
