//! Domain types. Plain data; no I/O.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::secret::Secret;

/// Milliseconds since the Unix epoch, as reported by a [`Clock`](crate::traits::Clock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(u64);

impl Timestamp {
    /// From milliseconds since the epoch.
    pub const fn from_millis(ms: u64) -> Self {
        Self(ms)
    }

    /// Milliseconds since the epoch.
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// This instant plus `d` (saturating).
    pub fn plus(self, d: Duration) -> Self {
        Self(
            self.0
                .saturating_add(u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        )
    }

    /// Time elapsed since `earlier`; zero if `earlier` is later.
    pub fn since(self, earlier: Self) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

/// A repository the token can see, as listed by GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// `owner/name`.
    pub full_name: String,
    /// Private or internal (anything but public).
    pub private: bool,
    /// Archived repos take no jobs.
    pub archived: bool,
    /// Disabled repos take no jobs.
    pub disabled: bool,
    /// Registering a runner needs repo admin.
    pub admin: bool,
}

impl Repo {
    /// The owner part of `owner/name`, as written.
    pub fn owner(&self) -> &str {
        owner_of(&self.full_name)
    }
}

/// The owner part of `owner/name`; the whole string if there is no slash.
pub fn owner_of(full_name: &str) -> &str {
    full_name.split_once('/').map_or(full_name, |(o, _)| o)
}

/// A job GitHub reports as queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedJob {
    /// `owner/name`.
    pub repo: String,
    /// Workflow run id.
    pub run_id: u64,
    /// Job id.
    pub job_id: u64,
    /// Workflow name, when known.
    pub workflow: Option<String>,
    /// Job name.
    pub name: String,
    /// `runs-on` labels as GitHub reports them.
    pub labels: Vec<String>,
    /// Browser URL of the run.
    pub html_url: String,
}

/// A runner process this app started and is tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningJob {
    /// Runner registration name (`ghm-...`).
    pub runner_name: String,
    /// `owner/name`.
    pub repo: String,
    /// Workflow name once the runner has taken a job.
    pub workflow: Option<String>,
    /// Job name once the runner has taken a job.
    pub job_name: Option<String>,
    /// Browser URL of the run once known.
    pub html_url: Option<String>,
    /// When the runner process started.
    pub started: Timestamp,
}

/// Queued jobs that cannot start because a cap is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingJobs {
    /// `owner/name`.
    pub repo: String,
    /// How many jobs wait.
    pub count: usize,
    /// Which cap holds them.
    pub reason: WaitReason,
}

/// Which cap holds a job back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    /// The per-owner cap.
    OwnerCap,
    /// The global cap.
    GlobalCap,
}

/// What the menu-bar icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// Nothing running, no error.
    Idle,
    /// At least one job running.
    Running,
    /// The last poll or spawn failed.
    Error,
    /// The user paused the app.
    Paused,
}

/// Everything the UI renders. Produced by the engine; the app only draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    /// The user paused the app.
    pub paused: bool,
    /// Runner processes in flight.
    pub running: Vec<RunningJob>,
    /// Jobs held back by caps.
    pub waiting: Vec<WaitingJobs>,
    /// Global cap, for the `n/max` display.
    pub max_runners: usize,
    /// Most recent error, without secrets.
    pub last_error: Option<String>,
    /// When this snapshot was taken.
    pub as_of: Timestamp,
}

impl StatusSnapshot {
    /// An empty, idle snapshot.
    pub fn idle(max_runners: usize, as_of: Timestamp) -> Self {
        Self {
            paused: false,
            running: Vec::new(),
            waiting: Vec::new(),
            max_runners,
            last_error: None,
            as_of,
        }
    }

    /// Paused beats error beats running beats idle.
    pub fn icon_state(&self) -> IconState {
        if self.paused {
            IconState::Paused
        } else if self.last_error.is_some() {
            IconState::Error
        } else if !self.running.is_empty() {
            IconState::Running
        } else {
            IconState::Idle
        }
    }

    /// Number of runners in flight.
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Total jobs held back by caps.
    pub fn waiting_count(&self) -> usize {
        self.waiting.iter().map(|w| w.count).sum()
    }
}

/// When to post a notification for a finished job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NotifyMode {
    /// Never.
    Off,
    /// Failed jobs only (default).
    #[default]
    FailuresOnly,
    /// Every finished job.
    All,
}

/// User preferences. Never holds the token (that is in the Keychain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Seconds between polls.
    pub poll_interval_secs: u64,
    /// Max concurrent runners per owner.
    pub per_owner_max: usize,
    /// Max concurrent runners overall.
    pub global_max: usize,
    /// Label set a job must fit inside (and include `macos`).
    pub labels: Vec<String>,
    /// If non-empty, serve only these `owner/name` repos.
    pub include: Vec<String>,
    /// Never serve these `owner/name` repos.
    pub exclude: Vec<String>,
    /// Public repos that may be served (default: none).
    pub public_allow: Vec<String>,
    /// Finished-job notifications.
    pub notify: NotifyMode,
    /// Start at login (LaunchAgent).
    pub launch_at_login: bool,
    /// Optional dedicated local user to run jobs as; unused in v1.
    pub job_user: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_secs: 30,
            per_owner_max: 2,
            global_max: 2,
            labels: ["self-hosted", "macos", "arm64"].map(String::from).to_vec(),
            include: Vec::new(),
            exclude: Vec::new(),
            public_allow: Vec::new(),
            notify: NotifyMode::default(),
            launch_at_login: false,
            job_user: None,
        }
    }
}

/// A just-in-time runner registration.
#[derive(Debug, Clone)]
pub struct JitConfig {
    /// GitHub's id for the registration, needed to delete it.
    pub runner_id: u64,
    /// `encoded_jit_config`, passed to `run.sh --jitconfig`. Secret.
    pub encoded: Secret<String>,
}

/// State of a registered runner as GitHub reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerInfo {
    /// GitHub's id.
    pub id: u64,
    /// Registration name.
    pub name: String,
    /// The runner is connected.
    pub online: bool,
    /// The runner is executing a job.
    pub busy: bool,
}

/// The latest `actions/runner` osx-arm64 release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerRelease {
    /// Version without a leading `v`.
    pub version: String,
    /// Asset download URL.
    pub url: String,
    /// Expected SHA-256 of the asset, lowercase hex.
    pub sha256: String,
}

/// Opaque id of a runner process held by a [`RunnerHost`](crate::traits::RunnerHost).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunnerHandle(pub u64);

/// State of a runner process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Still running.
    Running,
    /// Exited; `None` when killed by a signal.
    Exited(Option<i32>),
}

/// Where a workflow job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// Waiting for a runner.
    Queued,
    /// A runner is executing it.
    InProgress,
    /// Done; the conclusion says how it went.
    Completed,
}

/// How a completed job ended, as GitHub reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobConclusion {
    /// Green.
    Success,
    /// A step failed.
    Failure,
    /// Cancelled by a person or by a failing sibling.
    Cancelled,
    /// Hit its timeout.
    TimedOut,
    /// Skipped.
    Skipped,
    /// Neutral.
    Neutral,
    /// Waiting for approval.
    ActionRequired,
    /// A value this app does not know.
    Other,
}

impl JobConclusion {
    /// From GitHub's `conclusion` string.
    pub fn from_api(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            "failure" => Self::Failure,
            "cancelled" => Self::Cancelled,
            "timed_out" => Self::TimedOut,
            "skipped" => Self::Skipped,
            "neutral" => Self::Neutral,
            "action_required" => Self::ActionRequired,
            _ => Self::Other,
        }
    }

    /// A red job: failed or timed out. A cancellation is not counted (GitHub gives no way to tell a
    /// person's cancel from an error; a runner that dies mid-job is caught by its exit status instead).
    pub fn is_red(self) -> bool {
        matches!(self, Self::Failure | Self::TimedOut)
    }

    /// A few words for a notification.
    pub fn words(self) -> &'static str {
        match self {
            Self::Success => "succeeded",
            Self::Failure => "failed",
            Self::Cancelled => "was cancelled",
            Self::TimedOut => "timed out",
            Self::Skipped => "was skipped",
            Self::Neutral => "finished neutral",
            Self::ActionRequired => "needs action",
            Self::Other => "finished",
        }
    }
}

/// A workflow job as GitHub reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    /// `owner/name`.
    pub repo: String,
    /// Workflow run id.
    pub run_id: u64,
    /// Job id.
    pub job_id: u64,
    /// Workflow name, when known.
    pub workflow: Option<String>,
    /// Job name.
    pub name: String,
    /// Browser URL of the job.
    pub html_url: String,
    /// Where it is.
    pub status: JobStatus,
    /// How it ended, once completed.
    pub conclusion: Option<JobConclusion>,
}

/// A user-facing notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// Title line.
    pub title: String,
    /// Body text, without secrets.
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> RunningJob {
        RunningJob {
            runner_name: "ghm-x-abc123".into(),
            repo: "o/r".into(),
            workflow: None,
            job_name: None,
            html_url: None,
            started: Timestamp::from_millis(0),
        }
    }

    #[test]
    fn icon_state_priority() {
        let mut s = StatusSnapshot::idle(2, Timestamp::default());
        assert_eq!(s.icon_state(), IconState::Idle);
        s.running.push(job());
        assert_eq!(s.icon_state(), IconState::Running);
        s.last_error = Some("boom".into());
        assert_eq!(s.icon_state(), IconState::Error);
        s.paused = true;
        assert_eq!(s.icon_state(), IconState::Paused);
    }

    #[test]
    fn timestamp_arithmetic_saturates() {
        let t = Timestamp::from_millis(1_000);
        assert_eq!(t.plus(Duration::from_secs(2)).as_millis(), 3_000);
        assert_eq!(
            t.since(Timestamp::from_millis(400)),
            Duration::from_millis(600)
        );
        assert_eq!(t.since(Timestamp::from_millis(5_000)), Duration::ZERO);
        assert_eq!(
            Timestamp::from_millis(u64::MAX)
                .plus(Duration::from_secs(1))
                .as_millis(),
            u64::MAX
        );
    }

    #[test]
    fn owner_of_splits_on_first_slash() {
        assert_eq!(owner_of("Example-Org/app"), "Example-Org");
        assert_eq!(owner_of("solo"), "solo");
    }

    #[test]
    fn default_settings_serve_macos_only_and_no_public() {
        let s = Settings::default();
        assert_eq!(s.global_max, 2);
        assert!(s.public_allow.is_empty());
        assert!(s.labels.iter().any(|l| l == "macos"));
    }

    #[test]
    fn jit_config_debug_is_redacted() {
        let j = JitConfig {
            runner_id: 7,
            encoded: Secret::new("SENTINEL_JIT".into()),
        };
        assert!(!format!("{j:?}").contains("SENTINEL_JIT"));
    }
}
