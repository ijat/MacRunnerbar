//! Every fake passes its own contract suite.

use macrunnerbar_core::{
    JitConfig, JobConclusion, JobInfo, JobStatus, QueuedJob, Repo, Secret, Timestamp,
};
use macrunnerbar_testkit::contract::{self, GitHubFixture, JobFixture};
use macrunnerbar_testkit::{
    FakeClock, FakeGitHub, FakeKeychain, FakeLoginItem, FakeRunnerHost, MemSettingsStore,
    RecordingNotifier,
};

fn seeded_github() -> (FakeGitHub, GitHubFixture) {
    let gh = FakeGitHub::new();
    gh.add_repo(Repo {
        full_name: "owner/repo".into(),
        private: true,
        archived: false,
        disabled: false,
        admin: true,
    });
    gh.set_queued(
        "owner/repo",
        vec![QueuedJob {
            repo: "owner/repo".into(),
            run_id: 1,
            job_id: 42,
            workflow: Some("CI".into()),
            name: "build".into(),
            labels: vec!["self-hosted".into(), "macos".into(), "arm64".into()],
            html_url: "https://github.com/owner/repo/actions/runs/1".into(),
        }],
    );
    let fx = GitHubFixture {
        repo: "owner/repo".into(),
        queued_job_id: 42,
        unknown_repo: "owner/missing".into(),
        labels: vec!["self-hosted".into(), "macos".into(), "arm64".into()],
    };
    (gh, fx)
}

#[test]
fn fake_github_passes_contract() {
    let (gh, fx) = seeded_github();
    contract::github_contract(&gh, &fx);
}

#[test]
fn fake_github_passes_download_contract() {
    let gh = FakeGitHub::new();
    let release = gh_release(&gh);
    contract::github_download_contract(&gh, &release);
}

fn gh_release(gh: &FakeGitHub) -> macrunnerbar_core::RunnerRelease {
    use macrunnerbar_core::GitHubApi;
    gh.latest_runner_release().expect("fake release")
}

#[test]
fn fake_runner_host_passes_contract() {
    let host = FakeRunnerHost::with_template("2.0.0");
    let jit = JitConfig {
        runner_id: 1,
        encoded: Secret::new("jit".into()),
    };
    contract::runner_host_contract(&host, &jit);

    let dir = std::env::temp_dir().join(format!("mrb-fakehost-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let archive = dir.join("a.tar.gz");
    std::fs::write(&archive, b"x").expect("write");
    contract::runner_host_template_contract(&FakeRunnerHost::new(), &archive);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn fake_keychain_passes_contract() {
    contract::keychain_contract(&FakeKeychain::new());
}

#[test]
fn memory_settings_store_passes_contract() {
    contract::settings_store_contract(&MemSettingsStore::new());
}

#[test]
fn fake_clock_passes_contract() {
    contract::clock_contract(&FakeClock::new(Timestamp::from_millis(5)));
}

#[test]
fn recording_notifier_passes_contract() {
    let n = RecordingNotifier::new();
    contract::notifier_contract(&n);
    assert_eq!(n.sent().len(), 1);
}

#[test]
fn fake_login_item_passes_contract() {
    contract::login_item_contract(&FakeLoginItem::new());
}

#[test]
fn fake_github_passes_job_contract() {
    let (gh, _) = seeded_github();
    gh.add_job(
        JobInfo {
            repo: "owner/repo".into(),
            run_id: 5,
            job_id: 77,
            workflow: Some("CI".into()),
            name: "build".into(),
            html_url: "https://github.com/owner/repo/actions/runs/5/job/77".into(),
            status: JobStatus::Completed,
            conclusion: Some(JobConclusion::Failure),
        },
        "ghm-owner-repo-abc123",
    );
    contract::github_job_contract(
        &gh,
        &JobFixture {
            repo: "owner/repo".into(),
            runner_name: "ghm-owner-repo-abc123".into(),
            job_id: 77,
            conclusion: JobConclusion::Failure,
        },
    );
}
