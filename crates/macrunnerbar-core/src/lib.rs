//! MacRunnerbar core: domain types, traits for everything external, pure policy.
//!
//! No I/O, no UI toolkit, no HTTP client. Adapters (`macrunnerbar-github`, `macrunnerbar-macos`)
//! implement the traits; the app crate wires them together.

pub mod engine;
pub mod error;
pub mod policy;
pub mod secret;
pub mod traits;
pub mod types;

pub use error::{ApiError, OsError};
pub use secret::Secret;
pub use traits::{Clock, GitHubApi, Keychain, LoginItem, Notifier, RunnerHost, SettingsStore};
pub use types::{
    IconState, JitConfig, JobConclusion, JobInfo, JobStatus, Notification, NotifyMode,
    ProcessState, QueuedJob, Repo, RunnerHandle, RunnerInfo, RunnerRelease, RunningJob, Settings,
    StatusSnapshot, Timestamp, WaitReason, WaitingJobs, owner_of,
};
