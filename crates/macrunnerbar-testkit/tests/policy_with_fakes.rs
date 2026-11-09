//! Core policy driven through the fakes: the planning path the engine will take.

use macrunnerbar_core::policy::{BackoffPolicy, RepoBackoff};
use macrunnerbar_core::policy::{Load, RepoDemand, matches, plan_spawns};
use macrunnerbar_core::{ApiError, GitHubApi, QueuedJob, Repo, Settings, Timestamp, WaitReason};
use macrunnerbar_testkit::FakeGitHub;

fn job(id: u64, labels: &[&str]) -> QueuedJob {
    QueuedJob {
        repo: "o/r".into(),
        run_id: id,
        job_id: id,
        workflow: None,
        name: format!("job{id}"),
        labels: labels.iter().map(|l| (*l).to_string()).collect(),
        html_url: String::new(),
    }
}

fn gh_with(jobs: Vec<QueuedJob>) -> FakeGitHub {
    let gh = FakeGitHub::new();
    gh.add_repo(Repo {
        full_name: "o/r".into(),
        private: true,
        archived: false,
        disabled: false,
        admin: true,
    });
    gh.set_queued("o/r", jobs);
    gh
}

#[test]
fn three_queued_jobs_with_global_two_start_two_and_report_one_waiting() {
    let gh = gh_with(vec![
        job(1, &["self-hosted", "macos", "arm64"]),
        job(2, &["macos"]),
        job(3, &["self-hosted", "macOS"]),
        job(4, &["self-hosted"]), // bare self-hosted: Linux's, not ours
        job(5, &["ubuntu-latest"]),
    ]);
    let st = Settings {
        global_max: 2,
        per_owner_max: 8,
        ..Settings::default()
    };
    let queued = gh.queued_jobs("o/r").expect("queued");
    let ours = queued
        .iter()
        .filter(|j| matches(&j.labels, &st.labels))
        .count();
    assert_eq!(ours, 3);
    let demand = RepoDemand {
        repo: "o/r".into(),
        queued: ours,
        unclaimed: 0,
    };
    let plan = plan_spawns(&[demand], &Load::default(), st.per_owner_max, st.global_max);
    assert_eq!(plan.spawn.len(), 2);
    assert_eq!(plan.waiting.len(), 1);
    assert_eq!(plan.waiting[0].count, 1);
    assert_eq!(plan.waiting[0].reason, WaitReason::GlobalCap);
}

#[test]
fn injected_repo_failure_drives_backoff_and_recovery() {
    let gh = gh_with(vec![]);
    let mut backoff = RepoBackoff::new(BackoffPolicy::default());
    let t0 = Timestamp::from_millis(0);
    gh.fail_repo("o/r", ApiError::Http { status: 500 });
    assert!(gh.queued_jobs("o/r").is_err());
    let d1 = backoff.record_failure("o/r", t0);
    let d2 = backoff.record_failure("o/r", t0.plus(d1));
    assert_eq!(d2, d1 * 2);
    gh.clear_failures();
    assert!(gh.queued_jobs("o/r").is_ok());
    backoff.record_success("o/r");
    assert_eq!(backoff.failures("o/r"), 0);
}
