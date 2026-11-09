//! Contract suites: one function per trait. Each panics on a violation. The fakes pass them in this
//! crate's tests; every real adapter calls the same function from its own tests.
//!
//! Suites that need a seeded backend take a fixture or a pre-built host; each states what it expects.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use macrunnerbar_core::{
    ApiError, Clock, GitHubApi, JitConfig, Keychain, LoginItem, Notification, Notifier,
    ProcessState, RunnerHost, RunnerRelease, Secret, Settings, SettingsStore,
};

/// What a [`GitHubApi`] under test was seeded with.
#[derive(Debug, Clone)]
pub struct GitHubFixture {
    /// A repo the token can see and admin, with exactly the queued job below.
    pub repo: String,
    /// Id of a queued job in `repo`.
    pub queued_job_id: u64,
    /// A repo that does not exist.
    pub unknown_repo: String,
    /// Labels to request for the JIT runner.
    pub labels: Vec<String>,
}

fn scratch_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir =
        std::env::temp_dir().join(format!("mrb-contract-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Contract for [`GitHubApi`] (everything except `download_release`, see [`github_download_contract`]).
pub fn github_contract(api: &dyn GitHubApi, fx: &GitHubFixture) {
    let repos = api.list_repos().expect("list_repos");
    assert!(
        repos
            .iter()
            .any(|r| r.full_name.eq_ignore_ascii_case(&fx.repo)),
        "list_repos must include the fixture repo"
    );

    let jobs = api.queued_jobs(&fx.repo).expect("queued_jobs");
    let job = jobs
        .iter()
        .find(|j| j.job_id == fx.queued_job_id)
        .expect("fixture job is listed");
    assert!(!job.labels.is_empty(), "queued job carries its labels");
    assert!(
        matches!(api.queued_jobs(&fx.unknown_repo), Err(ApiError::NotFound)),
        "unknown repo is NotFound"
    );

    let name = "ghm-contract-abc123";
    let jit = api
        .generate_jit(&fx.repo, name, &fx.labels)
        .expect("generate_jit");
    assert!(jit.runner_id > 0, "JIT registration has an id");
    assert!(!jit.encoded.expose().is_empty(), "JIT config is non-empty");
    let runners = api.list_runners(&fx.repo).expect("list_runners");
    let reg = runners
        .iter()
        .find(|r| r.name == name)
        .expect("new registration is listed");
    assert_eq!(reg.id, jit.runner_id, "listed id matches the JIT id");

    api.delete_runner(&fx.repo, jit.runner_id)
        .expect("delete_runner");
    let runners = api
        .list_runners(&fx.repo)
        .expect("list_runners after delete");
    assert!(
        runners.iter().all(|r| r.name != name),
        "deleted registration is gone"
    );
    api.delete_runner(&fx.repo, jit.runner_id)
        .expect("delete_runner is idempotent");

    let rel = api.latest_runner_release().expect("latest_runner_release");
    assert!(
        !rel.version.is_empty() && !rel.version.starts_with('v'),
        "version has no leading v"
    );
    assert!(!rel.url.is_empty(), "release has a download url");
    assert!(
        rel.sha256.len() == 64
            && rel
                .sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "sha256 is 64 lowercase hex chars"
    );
}

/// Contract for `GitHubApi::download_release`. Downloads the real asset for a real adapter, so run it
/// only where a download is acceptable. `release` must be a genuine release with its genuine checksum.
pub fn github_download_contract(api: &dyn GitHubApi, release: &RunnerRelease) {
    let dir = scratch_dir("download");
    let dest = dir.join("runner.tar.gz");
    api.download_release(release, &dest)
        .expect("download with the right checksum");
    assert!(
        std::fs::metadata(&dest)
            .expect("downloaded file exists")
            .len()
            > 0
    );
    std::fs::remove_file(&dest).expect("remove download");

    let mut bad = release.clone();
    bad.sha256 = format!(
        "{}{}",
        if bad.sha256.starts_with('1') {
            '2'
        } else {
            '1'
        },
        &bad.sha256[1..]
    );
    let err = api
        .download_release(&bad, &dest)
        .expect_err("wrong checksum must fail");
    assert!(
        matches!(err, ApiError::Invalid(_)),
        "checksum mismatch is Invalid, got {err}"
    );
    assert!(!dest.exists(), "a failed download leaves no file");
    std::fs::remove_dir_all(&dir).expect("remove scratch dir");
}

/// Contract for the template half of [`RunnerHost`]. `archive` is a release archive the host accepts.
pub fn runner_host_template_contract(host: &dyn RunnerHost, archive: &Path) {
    host.install_template(archive, "9.9.9")
        .expect("install_template");
    assert_eq!(
        host.template_version()
            .expect("template_version")
            .as_deref(),
        Some("9.9.9")
    );
}

/// Contract for [`RunnerHost`]. Expects a host with a template installed whose runner stays up until
/// killed (a real host is seeded with a stub `run.sh` that sleeps).
pub fn runner_host_contract(host: &dyn RunnerHost, jit: &JitConfig) {
    assert!(
        host.template_version().expect("template_version").is_some(),
        "template installed"
    );
    let name = "ghm-contract-def456";
    let h = host.spawn(name, jit).expect("spawn");
    assert_eq!(host.poll(h).expect("poll"), ProcessState::Running);
    assert!(
        host.list_leftover()
            .expect("list_leftover")
            .iter()
            .any(|n| n == name)
    );

    host.kill(h).expect("kill");
    let deadline = Instant::now() + Duration::from_secs(10);
    while host.poll(h).expect("poll after kill") == ProcessState::Running {
        assert!(
            Instant::now() < deadline,
            "process still running 10s after kill"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    host.kill(h).expect("kill is idempotent");

    host.delete_dir(name).expect("delete_dir");
    assert!(
        host.list_leftover()
            .expect("list_leftover")
            .iter()
            .all(|n| n != name)
    );
    host.delete_dir(name).expect("delete_dir is idempotent");
}

/// Contract for [`Keychain`]. Touches the one token slot: use a scratch service name for a real adapter.
pub fn keychain_contract(kc: &dyn Keychain) {
    kc.delete_token().expect("delete on empty is Ok");
    assert!(kc.get_token().expect("get empty").is_none());
    kc.set_token(&Secret::new("tok-one".into())).expect("set");
    assert_eq!(
        kc.get_token().expect("get").expect("present").expose(),
        "tok-one"
    );
    kc.set_token(&Secret::new("tok-two".into()))
        .expect("overwrite");
    assert_eq!(
        kc.get_token().expect("get").expect("present").expose(),
        "tok-two"
    );
    kc.delete_token().expect("delete");
    assert!(kc.get_token().expect("get after delete").is_none());
    kc.delete_token().expect("delete is idempotent");
}

/// Contract for [`SettingsStore`]. Expects an empty store.
pub fn settings_store_contract(store: &dyn SettingsStore) {
    assert_eq!(
        store.load().expect("load empty"),
        Settings::default(),
        "empty store loads defaults"
    );
    let custom = Settings {
        poll_interval_secs: 45,
        per_owner_max: 3,
        global_max: 4,
        include: vec!["o/included".into()],
        exclude: vec!["o/excluded".into()],
        public_allow: vec!["o/public".into()],
        launch_at_login: true,
        ..Settings::default()
    };
    store.save(&custom).expect("save");
    assert_eq!(store.load().expect("load"), custom, "settings round-trip");
    store.save(&Settings::default()).expect("save defaults");
    assert_eq!(store.load().expect("load"), Settings::default());
}

/// Contract for [`Clock`].
pub fn clock_contract(clock: &dyn Clock) {
    let a = clock.now();
    clock.sleep(Duration::from_millis(20));
    let b = clock.now();
    assert!(b >= a, "time does not go backwards");
    assert!(
        b.since(a) >= Duration::from_millis(15),
        "sleep(20ms) advances at least ~20ms"
    );
}

/// Contract for [`Notifier`].
pub fn notifier_contract(n: &dyn Notifier) {
    n.notify(&Notification {
        title: "MacRunnerbar".into(),
        body: "contract test".into(),
    })
    .expect("notify");
}

/// Contract for [`LoginItem`]. Leaves it disabled.
pub fn login_item_contract(li: &dyn LoginItem) {
    li.set_enabled(false).expect("disable");
    assert!(!li.is_enabled().expect("is_enabled"));
    li.set_enabled(true).expect("enable");
    assert!(li.is_enabled().expect("is_enabled"));
    li.set_enabled(true).expect("enable is idempotent");
    li.set_enabled(false).expect("disable");
    assert!(!li.is_enabled().expect("is_enabled"));
    li.set_enabled(false).expect("disable is idempotent");
}
