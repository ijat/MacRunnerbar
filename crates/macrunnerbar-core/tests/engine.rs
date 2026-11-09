//! Engine behaviour with the fakes only: fake GitHub, fake runner host, fake clock. No real sleeping:
//! time moves by `FakeClock::advance`.

mod engine {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use macrunnerbar_core::engine::{Deps, Engine, EngineConfig, EngineHandle, Observer};
    use macrunnerbar_core::{
        ApiError, IconState, JobConclusion, JobInfo, JobStatus, NotifyMode, OsError, QueuedJob,
        Repo, RunnerHost, RunnerRelease, Settings, SettingsStore, StatusSnapshot, WaitReason,
    };
    use macrunnerbar_testkit::{
        FakeClock, FakeGitHub, FakeRunnerHost, MemSettingsStore, RecordingNotifier,
    };

    const LABELS: [&str; 3] = ["self-hosted", "macos", "arm64"];

    struct Rig {
        gh: Arc<FakeGitHub>,
        host: Arc<FakeRunnerHost>,
        clock: Arc<FakeClock>,
        notifier: Arc<RecordingNotifier>,
        store: Arc<MemSettingsStore>,
        engine: Engine,
        scratch: PathBuf,
        queues: std::collections::HashMap<String, Vec<QueuedJob>>,
    }

    fn unique_scratch() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("mrb-engine-{}-{nanos}", std::process::id()))
    }

    fn base_settings() -> Settings {
        Settings {
            global_max: 2,
            per_owner_max: 2,
            poll_interval_secs: 30,
            ..Settings::default()
        }
    }

    fn rig_with(settings: Settings, host: FakeRunnerHost, repos: &[(&str, bool)]) -> Rig {
        let gh = Arc::new(FakeGitHub::new());
        for (name, private) in repos {
            gh.add_repo(Repo {
                full_name: (*name).into(),
                private: *private,
                archived: false,
                disabled: false,
                admin: true,
            });
        }
        let host = Arc::new(host);
        let clock = Arc::new(FakeClock::default());
        let notifier = Arc::new(RecordingNotifier::new());
        let store = Arc::new(MemSettingsStore::new());
        store.save(&settings).expect("save settings");
        let scratch = unique_scratch();
        let deps = Deps {
            github: gh.clone(),
            host: host.clone(),
            clock: clock.clone(),
            notifier: notifier.clone(),
            settings: store.clone(),
        };
        let engine = Engine::new(deps, EngineConfig::new(&scratch));
        Rig {
            gh,
            host,
            clock,
            notifier,
            store,
            engine,
            scratch,
            queues: Default::default(),
        }
    }

    /// Two private repos `o/r`; template already at the fake release version, so no download happens.
    fn rig() -> Rig {
        rig_with(
            base_settings(),
            FakeRunnerHost::with_template("2.999.0"),
            &[("o/r", true)],
        )
    }

    fn job(repo: &str, id: u64, labels: &[&str]) -> QueuedJob {
        QueuedJob {
            repo: repo.into(),
            run_id: id,
            job_id: id,
            workflow: Some("CI".into()),
            name: format!("job-{id}"),
            labels: labels.iter().map(|l| (*l).to_string()).collect(),
            html_url: format!("https://github.com/{repo}/actions/runs/{id}"),
        }
    }

    impl Rig {
        fn queue(&mut self, repo: &str, ids: &[u64]) {
            let jobs: Vec<QueuedJob> = ids.iter().map(|i| job(repo, *i, &LABELS)).collect();
            self.gh.set_queued(repo, jobs.clone());
            self.queues.insert(repo.into(), jobs);
        }

        fn queue_jobs(&mut self, repo: &str, jobs: Vec<QueuedJob>) {
            self.gh.set_queued(repo, jobs.clone());
            self.queues.insert(repo.into(), jobs);
        }

        /// A runner takes a job: GitHub marks it busy and the job leaves the queue.
        fn claim(&mut self, runner: &str, repo: &str, job_id: u64) {
            self.gh.set_runner_busy(repo, runner, true);
            let q = self.queues.entry(repo.into()).or_default();
            q.retain(|j| j.job_id != job_id);
            self.gh.set_queued(repo, q.clone());
        }

        fn tick(&mut self) {
            self.engine.tick();
        }

        fn advance(&mut self, secs: u64) {
            self.clock.advance(Duration::from_secs(secs));
        }

        fn tick_after(&mut self, secs: u64) {
            self.advance(secs);
            self.tick();
        }

        fn calls(&self, prefix: &str) -> usize {
            self.gh
                .calls()
                .iter()
                .filter(|c| c.starts_with(prefix))
                .count()
        }

        fn names(&self) -> Vec<String> {
            self.host.spawned()
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.scratch);
        }
    }

    #[test]
    fn three_queued_jobs_with_cap_two_start_two_and_report_one_waiting() {
        // Per-owner cap above the global one, so the global cap is the one that binds.
        let s = Settings {
            per_owner_max: 8,
            ..base_settings()
        };
        let mut r = rig_with(
            s,
            FakeRunnerHost::with_template("2.999.0"),
            &[("o/r", true)],
        );
        r.queue("o/r", &[1, 2, 3]);
        r.tick();
        assert_eq!(r.names().len(), 2);
        let snap = r.engine.snapshot();
        assert_eq!(snap.running.len(), 2);
        assert_eq!(snap.waiting.len(), 1);
        assert_eq!(
            (snap.waiting[0].repo.as_str(), snap.waiting[0].count),
            ("o/r", 1)
        );
        assert_eq!(snap.waiting[0].reason, WaitReason::GlobalCap);
        assert_eq!(snap.max_runners, 2);
        assert_eq!(snap.icon_state(), IconState::Running);
        assert_eq!(
            r.gh.runners("o/r").len(),
            2,
            "one JIT registration per runner"
        );
        assert!(r.names().iter().all(|n| n.starts_with("ghm-o-r-")));
    }

    #[test]
    fn finished_job_frees_a_slot_on_the_next_tick() {
        let mut r = rig();
        r.queue("o/r", &[1, 2, 3]);
        r.tick();
        let (a, b) = (r.names()[0].clone(), r.names()[1].clone());
        r.claim(&a, "o/r", 1);
        r.tick_after(30); // scan: A is busy, jobs 2 and 3 wait (B is idle and will take one)
        assert_eq!(r.names().len(), 2);
        assert_eq!(r.engine.snapshot().waiting[0].count, 1);

        r.host.finish(&a, Some(0));
        r.tick(); // no clock movement: the freed slot triggers an immediate rescan
        let names = r.names();
        assert_eq!(names.len(), 3, "a third runner started in the freed slot");
        let snap = r.engine.snapshot();
        assert_eq!(snap.running.len(), 2);
        assert!(snap.running.iter().all(|j| j.runner_name != a));
        assert!(snap.running.iter().any(|j| j.runner_name == b));
        assert!(snap.waiting.is_empty());
        assert!(
            !r.host.list_leftover().expect("leftover").contains(&a),
            "A's directory was deleted"
        );
        assert!(
            r.gh.runners("o/r").iter().all(|x| x.name != a),
            "A's registration was deleted"
        );
    }

    #[test]
    fn paused_engine_starts_nothing_but_keeps_tracking_running_jobs() {
        let mut r = rig();
        r.queue("o/r", &[1, 2, 3]);
        r.tick();
        let a = r.names()[0].clone();
        r.engine.set_paused(true);
        r.claim(&a, "o/r", 1);
        r.tick();
        let snap = r.engine.snapshot();
        assert!(snap.paused);
        assert_eq!(snap.icon_state(), IconState::Paused);
        assert_eq!(snap.running.len(), 2, "still tracking both runners");
        assert!(snap.waiting.is_empty());

        r.host.finish(&a, Some(0));
        r.tick_after(30);
        assert_eq!(
            r.engine.snapshot().running.len(),
            1,
            "the finished runner is reaped while paused"
        );
        assert_eq!(r.names().len(), 2, "but nothing new was started");
        assert!(!r.host.list_leftover().expect("leftover").contains(&a));

        r.engine.set_paused(false);
        r.tick();
        assert_eq!(r.names().len(), 3, "resume starts work again");
        assert!(!r.engine.snapshot().paused);
    }

    #[test]
    fn shutdown_kills_every_child_and_removes_every_directory_and_registration() {
        let mut r = rig();
        r.queue("o/r", &[1, 2]);
        r.tick();
        assert_eq!(r.host.running().len(), 2);
        r.engine.shutdown();
        assert!(r.host.running().is_empty(), "no process left");
        assert_eq!(r.host.killed().len(), 2);
        assert!(
            r.host.list_leftover().expect("leftover").is_empty(),
            "no job directory left"
        );
        assert!(r.gh.runners("o/r").is_empty(), "no registration left");
        assert!(r.engine.snapshot().running.is_empty());
    }

    #[test]
    fn an_api_error_backs_off_only_that_repo() {
        let mut r = rig_with(
            base_settings(),
            FakeRunnerHost::with_template("2.999.0"),
            &[("a/x", true), ("b/y", true)],
        );
        r.queue("a/x", &[1]);
        r.queue("b/y", &[2]);
        r.gh.fail_repo("a/x", ApiError::Http { status: 500 });
        r.tick();
        assert!(
            r.names().iter().any(|n| n.starts_with("ghm-b-y-")),
            "b/y is served"
        );
        assert!(!r.names().iter().any(|n| n.starts_with("ghm-a-x-")));
        let err = r.engine.snapshot().last_error.expect("error shown");
        assert!(err.contains("a/x"), "{err}");
        assert_eq!(r.engine.snapshot().icon_state(), IconState::Error);
        assert_eq!(r.calls("queued_jobs a/x"), 1);

        let b_calls = r.calls("queued_jobs b/y");
        r.tick_after(30);
        assert_eq!(
            r.calls("queued_jobs a/x"),
            1,
            "still backed off after 30 s (60 s delay)"
        );
        assert!(
            r.calls("queued_jobs b/y") > b_calls,
            "b/y keeps being polled"
        );

        r.tick_after(31);
        assert_eq!(
            r.calls("queued_jobs a/x"),
            2,
            "retried once the delay passed"
        );
        // The second failure doubles the delay to 120 s.
        r.tick_after(60);
        assert_eq!(r.calls("queued_jobs a/x"), 2);

        r.gh.clear_failures();
        r.tick_after(61);
        assert_eq!(r.calls("queued_jobs a/x"), 3);
        assert!(
            r.names().iter().any(|n| n.starts_with("ghm-a-x-")),
            "a/x served after recovery"
        );
        assert!(
            r.engine.snapshot().last_error.is_none(),
            "error cleared by the success"
        );
    }

    #[test]
    fn startup_removes_leftover_dirs_and_the_sweep_deletes_only_stray_ghm_registrations() {
        let mut r = rig();
        r.host.add_leftover("ghm-old-aaaaaa");
        r.host.add_leftover("gha-foreign-dir");
        r.gh.add_runner("o/r", "ghm-stale-bbbbbb", false, false);
        r.gh.add_runner("o/r", "gha-linux-1", false, false);
        r.gh.add_runner("o/r", "ghm-live-cccccc", true, true);
        r.gh.add_runner("o/r", "ghm-connecting-dddddd", true, false);
        r.engine.startup();
        let left = r.host.list_leftover().expect("leftover");
        assert_eq!(
            left,
            vec!["gha-foreign-dir"],
            "only ghm- directories are removed"
        );
        r.tick();
        let names: Vec<String> = r.gh.runners("o/r").into_iter().map(|x| x.name).collect();
        assert!(
            !names.contains(&"ghm-stale-bbbbbb".to_string()),
            "stray registration deleted"
        );
        for keep in ["gha-linux-1", "ghm-live-cccccc", "ghm-connecting-dddddd"] {
            assert!(
                names.contains(&keep.to_string()),
                "{keep} must not be touched"
            );
        }
    }

    #[test]
    fn network_loss_is_account_wide_backs_off_and_recovers() {
        let mut r = rig_with(
            base_settings(),
            FakeRunnerHost::with_template("2.999.0"),
            &[("a/x", true), ("b/y", true)],
        );
        r.queue("a/x", &[1]);
        r.queue("b/y", &[2]);
        r.gh.fail_repo("a/x", ApiError::Network("connection failed".into()));
        r.gh.fail_repo("b/y", ApiError::Network("connection failed".into()));
        r.tick();
        assert_eq!(
            r.calls("queued_jobs"),
            1,
            "the first network error stops the scan"
        );
        let snap = r.engine.snapshot();
        assert_eq!(snap.icon_state(), IconState::Error);
        assert!(snap.last_error.expect("error").contains("network"));
        assert!(r.names().is_empty());

        r.tick_after(30);
        assert_eq!(
            r.calls("queued_jobs"),
            1,
            "no API traffic during the 60 s backoff"
        );
        r.tick_after(31);
        assert_eq!(
            r.calls("queued_jobs"),
            2,
            "one probe after the backoff (the failure doubles it to 120 s)"
        );
        r.tick_after(90);
        assert_eq!(r.calls("queued_jobs"), 2);

        r.gh.clear_failures();
        r.tick_after(31);
        assert_eq!(r.names().len(), 2, "both repos served after recovery");
        assert!(r.engine.snapshot().last_error.is_none());
    }

    #[test]
    fn running_runners_are_reaped_during_an_outage_and_their_registration_is_deleted_after() {
        let mut r = rig();
        r.queue("o/r", &[1]);
        r.tick();
        let a = r.names()[0].clone();
        r.claim(&a, "o/r", 1);
        r.gh.fail_repo("o/r", ApiError::Network("down".into()));
        r.host.finish(&a, Some(0));
        r.tick_after(30);
        assert!(
            r.engine.snapshot().running.is_empty(),
            "reaped even though GitHub is unreachable"
        );
        assert!(
            !r.host.list_leftover().expect("leftover").contains(&a),
            "directory removed locally"
        );
        assert!(
            r.gh.runners("o/r").iter().any(|x| x.name == a),
            "registration still there"
        );
        r.gh.clear_failures();
        r.tick_after(200);
        assert!(
            r.gh.runners("o/r").iter().all(|x| x.name != a),
            "registration deleted once reachable"
        );
    }

    #[test]
    fn unauthorized_and_rate_limit_are_account_wide() {
        let mut r = rig();
        r.queue("o/r", &[1]);
        r.gh.fail_repo("o/r", ApiError::Unauthorized);
        r.tick();
        assert!(
            r.engine
                .snapshot()
                .last_error
                .expect("error")
                .contains("token")
        );
        r.gh.clear_failures();
        r.gh.fail_repo(
            "o/r",
            ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(300)),
            },
        );
        r.tick_after(61);
        assert!(
            r.engine
                .snapshot()
                .last_error
                .expect("error")
                .contains("rate limit")
        );
        let calls = r.calls("queued_jobs");
        r.gh.clear_failures();
        // Steps stay under the wake gap (120 s) so this is not mistaken for a sleep.
        r.tick_after(100);
        r.tick_after(100);
        assert_eq!(r.calls("queued_jobs"), calls, "honours retry-after (300 s)");
        r.tick_after(101);
        assert_eq!(r.names().len(), 1, "served after the limit expired");
    }

    #[test]
    fn a_long_gap_is_treated_as_a_wake_from_sleep_and_rescans_with_cleared_backoff() {
        let mut r = rig();
        r.queue("o/r", &[1]);
        r.gh.fail_repo("o/r", ApiError::Http { status: 500 });
        r.tick();
        assert_eq!(r.calls("queued_jobs"), 1);
        r.gh.clear_failures();
        r.tick_after(30);
        assert_eq!(
            r.calls("queued_jobs"),
            1,
            "a short gap changes nothing: still backed off"
        );
        r.tick_after(2 * 3600); // the Mac slept
        assert_eq!(r.calls("queued_jobs"), 2, "rescanned immediately");
        assert_eq!(r.names().len(), 1);
        assert!(r.engine.snapshot().last_error.is_none());
    }

    #[test]
    fn an_idle_runner_is_stopped_after_the_timeout_and_a_busy_one_never() {
        let mut r = rig();
        r.queue("o/r", &[1]);
        r.tick();
        let a = r.names()[0].clone();
        r.queue("o/r", &[]); // the job went to another runner
        r.tick_after(300);
        assert_eq!(
            r.engine.snapshot().running.len(),
            1,
            "not yet: 300 s < 600 s"
        );
        r.tick_after(301);
        assert!(
            r.engine.snapshot().running.is_empty(),
            "stopped after the idle timeout"
        );
        assert_eq!(r.host.killed(), vec![a.clone()]);
        assert!(!r.host.list_leftover().expect("leftover").contains(&a));
        assert!(
            r.gh.runners("o/r").iter().all(|x| x.name != a),
            "registration deleted"
        );

        // A runner that took a job is left alone however long the job runs.
        r.queue("o/r", &[2]);
        r.tick_after(31);
        let b = r.names()[1].clone();
        r.claim(&b, "o/r", 2);
        r.tick_after(31);
        for _ in 0..10 {
            r.tick_after(600);
        }
        assert_eq!(r.engine.snapshot().running.len(), 1);
        assert_eq!(r.host.running(), vec![b]);
    }

    #[test]
    fn an_unclaimed_runner_counts_against_demand_so_no_double_spawn() {
        let mut r = rig();
        r.queue("o/r", &[1]);
        r.tick();
        r.tick_after(30);
        r.tick_after(30);
        assert_eq!(
            r.names().len(),
            1,
            "the job is still queued but a runner is already on its way"
        );
    }

    #[test]
    fn label_public_include_and_exclude_policy_is_applied() {
        let mut r = rig_with(
            base_settings(),
            FakeRunnerHost::with_template("2.999.0"),
            &[
                ("o/priv", true),
                ("o/pub", false),
                ("o/excl", true),
                ("o/other", true),
            ],
        );
        r.queue_jobs(
            "o/priv",
            vec![
                job("o/priv", 1, &["self-hosted"]),
                job("o/priv", 2, &["ubuntu-latest"]),
            ],
        );
        r.queue("o/pub", &[3]);
        r.queue("o/excl", &[4]);
        let mut s = base_settings();
        s.exclude = vec!["O/Excl".into()];
        r.store.save(&s).expect("save");
        r.engine.request_scan();
        r.tick();
        assert!(
            r.names().is_empty(),
            "bare self-hosted, ubuntu, public and excluded are all skipped"
        );
        assert_eq!(
            r.calls("queued_jobs o/pub"),
            0,
            "a public repo is not even polled"
        );
        assert_eq!(r.calls("queued_jobs o/excl"), 0);

        s.public_allow = vec!["o/pub".into()];
        r.store.save(&s).expect("save");
        r.engine.request_scan();
        r.tick();
        assert_eq!(r.names().len(), 1);
        assert!(r.names()[0].starts_with("ghm-o-pub-"));

        s.include = vec!["o/other".into()];
        r.store.save(&s).expect("save");
        r.queue("o/other", &[5]);
        r.engine.request_scan();
        r.tick();
        assert!(
            r.names().iter().any(|n| n.starts_with("ghm-o-other-")),
            "include list serves only o/other"
        );
    }

    #[test]
    fn per_owner_cap_is_applied_across_repos_before_the_global_cap() {
        let mut s = base_settings();
        s.per_owner_max = 1;
        s.global_max = 5;
        let mut r = rig_with(
            s,
            FakeRunnerHost::with_template("2.999.0"),
            &[("a/x", true), ("a/y", true), ("b/z", true)],
        );
        r.queue("a/x", &[1]);
        r.queue("a/y", &[2]);
        r.queue("b/z", &[3]);
        r.tick();
        assert_eq!(r.names().len(), 2, "one for owner a, one for owner b");
        let snap = r.engine.snapshot();
        assert_eq!(snap.waiting.len(), 1);
        assert_eq!(snap.waiting[0].reason, WaitReason::OwnerCap);
    }

    #[test]
    fn a_failed_spawn_deletes_the_registration_and_backs_the_repo_off() {
        let mut r = rig();
        r.queue("o/r", &[1]);
        r.host.fail_next_spawn(OsError::new("disk full"));
        r.tick();
        assert!(
            r.gh.runners("o/r").is_empty(),
            "no registration left behind"
        );
        assert!(
            r.engine
                .snapshot()
                .last_error
                .expect("error")
                .contains("cannot start a runner")
        );
        r.tick_after(30);
        assert_eq!(r.calls("queued_jobs"), 1, "backed off");
        r.tick_after(31);
        assert_eq!(r.names().len(), 1, "retried and started");
        assert!(r.engine.snapshot().last_error.is_none());
    }

    #[test]
    fn crashing_runners_back_the_repo_off_after_three_in_a_row_and_failures_notify() {
        let mut s = base_settings();
        s.global_max = 1;
        let mut r = rig_with(
            s,
            FakeRunnerHost::with_template("2.999.0"),
            &[("o/r", true)],
        );
        r.queue("o/r", &[1]);
        r.tick();
        for i in 0..3 {
            let name = r.names()[i].clone();
            r.host.finish(&name, Some(1));
            r.tick_after(30);
        }
        assert_eq!(r.notifier.sent().len(), 3, "failures notify by default");
        assert!(r.notifier.sent()[0].body.contains("o/r"));
        let err = r.engine.snapshot().last_error.expect("error");
        assert!(err.contains("3 runners in a row failed"), "{err}");
        assert_eq!(r.names().len(), 3, "no fourth runner while backed off");
        let calls = r.calls("queued_jobs");
        r.tick_after(30);
        assert_eq!(r.calls("queued_jobs"), calls);
    }

    #[test]
    fn notification_modes() {
        for (mode, exit, expected) in [
            (NotifyMode::FailuresOnly, 0, 0),
            (NotifyMode::FailuresOnly, 2, 1),
            (NotifyMode::All, 0, 1),
            (NotifyMode::Off, 2, 0),
        ] {
            let mut s = base_settings();
            s.notify = mode;
            let mut r = rig_with(
                s,
                FakeRunnerHost::with_template("2.999.0"),
                &[("o/r", true)],
            );
            r.queue("o/r", &[1]);
            r.tick();
            let a = r.names()[0].clone();
            r.host.finish(&a, Some(exit));
            r.tick();
            assert_eq!(r.notifier.sent().len(), expected, "{mode:?} exit {exit}");
        }
    }

    #[test]
    fn template_is_installed_before_the_first_runner_and_refreshed_daily() {
        let mut r = rig_with(base_settings(), FakeRunnerHost::new(), &[("o/r", true)]);
        r.queue("o/r", &[1]);
        r.tick();
        assert_eq!(r.calls("download_release"), 1);
        assert_eq!(
            r.names().len(),
            1,
            "installed first, then a runner started in the same tick"
        );
        assert_eq!(
            macrunnerbar_core::RunnerHost::template_version(&*r.host)
                .expect("version")
                .as_deref(),
            Some("2.999.0")
        );
        assert!(
            std::fs::read_dir(&r.scratch).map_or(true, |mut d| d.next().is_none()),
            "the downloaded archive is removed"
        );

        r.gh.set_release(RunnerRelease {
            version: "3.0.0".into(),
            url: "https://example.invalid/x.tar.gz".into(),
            sha256: "0".repeat(64),
        });
        r.tick_after(23 * 3600);
        assert_eq!(r.calls("latest_runner_release"), 1, "no check before 24 h");
        r.tick_after(2 * 3600);
        assert_eq!(r.calls("latest_runner_release"), 2);
        assert_eq!(
            macrunnerbar_core::RunnerHost::template_version(&*r.host)
                .expect("version")
                .as_deref(),
            Some("3.0.0")
        );
    }

    #[test]
    fn without_a_template_nothing_starts_and_the_update_is_retried_hourly() {
        let mut r = rig_with(base_settings(), FakeRunnerHost::new(), &[("o/r", true)]);
        r.queue("o/r", &[1]);
        r.gh.fail_release(ApiError::Http { status: 502 });
        r.tick();
        assert!(r.names().is_empty());
        assert!(
            r.engine
                .snapshot()
                .last_error
                .expect("error")
                .contains("runner template")
        );
        r.tick_after(31);
        assert_eq!(
            r.calls("latest_runner_release"),
            1,
            "not retried within the hour"
        );
        r.gh.clear_failures();
        r.tick_after(3600);
        assert_eq!(r.calls("latest_runner_release"), 2);
        r.tick();
        assert_eq!(r.names().len(), 1);
        assert!(r.engine.snapshot().last_error.is_none());
    }

    #[test]
    fn a_failed_refresh_of_a_working_template_keeps_it_and_is_not_an_error() {
        let mut r = rig();
        r.gh.fail_release(ApiError::Network("down".into()));
        r.tick();
        assert!(r.engine.snapshot().last_error.is_none());
        assert_eq!(
            macrunnerbar_core::RunnerHost::template_version(&*r.host)
                .expect("version")
                .as_deref(),
            Some("2.999.0")
        );
    }

    #[test]
    fn repos_take_turns_when_slots_are_scarce() {
        let mut s = base_settings();
        s.global_max = 1;
        let mut r = rig_with(
            s,
            FakeRunnerHost::with_template("2.999.0"),
            &[("a/x", true), ("b/y", true)],
        );
        r.queue("a/x", &[1]);
        r.queue("b/y", &[2]);
        r.tick();
        let first = r.names()[0].clone();
        r.host.finish(&first, Some(0));
        r.tick_after(30);
        let second = r.names()[1].clone();
        assert_ne!(
            first[..8],
            second[..8],
            "{first} then {second}: both repos got a turn"
        );
    }

    #[test]
    fn snapshot_shows_job_details_once_a_runner_has_taken_a_job() {
        let mut r = rig();
        r.queue("o/r", &[7]);
        r.tick();
        let a = r.names()[0].clone();
        assert_eq!(
            r.engine.snapshot().running[0].job_name,
            None,
            "not shown while still waiting for a job"
        );
        r.claim(&a, "o/r", 7);
        r.tick_after(30);
        let j = &r.engine.snapshot().running[0];
        assert_eq!(j.repo, "o/r");
        assert_eq!(j.job_name.as_deref(), Some("job-7"));
        assert_eq!(j.workflow.as_deref(), Some("CI"));
        assert_eq!(
            j.html_url.as_deref(),
            Some("https://github.com/o/r/actions/runs/7")
        );
        assert_eq!(
            j.started.as_millis(),
            r.engine.snapshot().running[0].started.as_millis()
        );
    }

    #[test]
    fn registrations_are_only_ever_created_with_the_ghm_prefix() {
        let mut r = rig();
        r.queue("o/r", &[1, 2]);
        r.tick();
        assert!(
            r.gh.runners("o/r")
                .iter()
                .all(|x| x.name.starts_with("ghm-"))
        );
    }

    // ---- job conclusions ------------------------------------------------------------------------

    fn job_info(id: u64, status: JobStatus, conclusion: Option<JobConclusion>) -> JobInfo {
        JobInfo {
            repo: "o/r".into(),
            run_id: 100 + id,
            job_id: id,
            workflow: Some("CI".into()),
            name: "build".into(),
            html_url: format!("https://github.com/o/r/actions/runs/{}/job/{id}", 100 + id),
            status,
            conclusion,
        }
    }

    /// A runner that has taken job 1; the engine has seen it busy and learned the job.
    fn busy_runner(r: &mut Rig) -> String {
        r.queue("o/r", &[1]);
        r.tick();
        let a = r.names()[0].clone();
        r.claim(&a, "o/r", 1);
        r.gh.add_job(job_info(1, JobStatus::InProgress, None), &a);
        r.tick_after(30);
        a
    }

    #[test]
    fn a_red_job_notifies_even_though_the_runner_exits_cleanly() {
        let mut r = rig();
        let a = busy_runner(&mut r);
        let snap = r.engine.snapshot();
        assert_eq!(
            snap.running[0].job_name.as_deref(),
            Some("build"),
            "exact job once found"
        );
        assert!(
            snap.running[0]
                .html_url
                .as_deref()
                .is_some_and(|u| u.ends_with("/job/1"))
        );

        r.gh.set_job_state(1, JobStatus::Completed, Some(JobConclusion::Failure));
        r.host.finish(&a, Some(0));
        r.tick();
        let sent = r.notifier.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].title, "Job failed");
        assert!(
            sent[0].body.contains("o/r") && sent[0].body.contains("CI / build"),
            "{}",
            sent[0].body
        );
        assert_eq!(
            r.calls("get_job"),
            1,
            "one extra request for the finished job"
        );
    }

    #[test]
    fn green_cancelled_and_skipped_jobs_do_not_notify_under_failures_only_but_timeouts_do() {
        for (conclusion, expected) in [
            (JobConclusion::Success, 0),
            (JobConclusion::Cancelled, 0),
            (JobConclusion::Skipped, 0),
            (JobConclusion::TimedOut, 1),
            (JobConclusion::Failure, 1),
        ] {
            let mut r = rig();
            let a = busy_runner(&mut r);
            r.gh.set_job_state(1, JobStatus::Completed, Some(conclusion));
            r.host.finish(&a, Some(0));
            r.tick();
            assert_eq!(r.notifier.sent().len(), expected, "{conclusion:?}");
        }
    }

    #[test]
    fn all_mode_reports_a_green_job_with_its_conclusion() {
        let s = Settings {
            notify: NotifyMode::All,
            ..base_settings()
        };
        let mut r = rig_with(
            s,
            FakeRunnerHost::with_template("2.999.0"),
            &[("o/r", true)],
        );
        let a = busy_runner(&mut r);
        r.gh.set_job_state(1, JobStatus::Completed, Some(JobConclusion::Success));
        r.host.finish(&a, Some(0));
        r.tick();
        let sent = r.notifier.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].title, "Job finished");
        assert!(sent[0].body.contains("succeeded"), "{}", sent[0].body);
    }

    #[test]
    fn a_conclusion_that_is_not_there_yet_is_asked_again_and_reported_once() {
        let mut r = rig();
        let a = busy_runner(&mut r);
        r.host.finish(&a, Some(0));
        r.tick(); // the job is still "in progress" on GitHub's side
        assert!(r.notifier.sent().is_empty());
        r.tick_after(5);
        assert_eq!(r.calls("get_job"), 1, "not asked again within 10 s");
        r.gh.set_job_state(1, JobStatus::Completed, Some(JobConclusion::Failure));
        r.tick_after(6);
        assert_eq!(r.notifier.sent().len(), 1);
        r.tick_after(30);
        r.tick_after(30);
        assert_eq!(r.notifier.sent().len(), 1, "never twice");
    }

    #[test]
    fn a_failed_lookup_is_retried_and_after_five_minutes_a_crash_is_reported_without_it() {
        let mut r = rig();
        let a = busy_runner(&mut r);
        r.gh.fail_repo("o/r", ApiError::Http { status: 500 });
        r.host.finish(&a, Some(0));
        r.tick();
        r.gh.clear_failures();
        r.gh.set_job_state(1, JobStatus::Completed, Some(JobConclusion::Failure));
        r.tick_after(11);
        assert_eq!(r.notifier.sent().len(), 1, "retried and reported");

        // The job never completes on GitHub's side: the runner's own failure is reported after 5 min.
        let mut r = rig();
        let a = busy_runner(&mut r);
        r.host.finish(&a, Some(1));
        r.tick();
        let mut waited = 0;
        while waited < 330 {
            r.tick_after(10);
            waited += 10;
        }
        let sent = r.notifier.sent();
        assert_eq!(sent.len(), 1, "one notification, however long it took");
        assert_eq!(sent[0].title, "Runner failed");
    }

    #[test]
    fn a_job_that_ended_before_the_engine_saw_it_busy_is_still_found_by_runner_name() {
        let mut r = rig();
        r.queue("o/r", &[1]);
        r.tick();
        let a = r.names()[0].clone();
        r.gh.add_job(
            job_info(1, JobStatus::Completed, Some(JobConclusion::Failure)),
            &a,
        );
        r.host.finish(&a, Some(0));
        r.tick();
        assert_eq!(r.notifier.sent().len(), 1);
        assert_eq!(r.notifier.sent()[0].title, "Job failed");
    }

    #[test]
    fn notifications_off_never_asks_for_a_job() {
        let s = Settings {
            notify: NotifyMode::Off,
            ..base_settings()
        };
        let mut r = rig_with(
            s,
            FakeRunnerHost::with_template("2.999.0"),
            &[("o/r", true)],
        );
        let a = busy_runner(&mut r);
        let before = r.calls("get_job") + r.calls("find_job_by_runner");
        r.gh.set_job_state(1, JobStatus::Completed, Some(JobConclusion::Failure));
        r.host.finish(&a, Some(1));
        r.tick();
        assert!(r.notifier.sent().is_empty());
        assert_eq!(r.calls("get_job") + r.calls("find_job_by_runner"), before);
    }

    // ---- the threaded handle ------------------------------------------------------------------

    #[test]
    fn handle_runs_ticks_pauses_notifies_and_shuts_down_cleanly() {
        let r = rig();
        let mut r = r;
        r.queue("o/r", &[1, 2]);
        let (gh, host, clock, notifier, store) = (
            r.gh.clone(),
            r.host.clone(),
            r.clock.clone(),
            r.notifier.clone(),
            r.store.clone(),
        );
        let engine = Engine::new(
            Deps {
                github: gh.clone(),
                host: host.clone(),
                clock,
                notifier,
                settings: store,
            },
            EngineConfig::new(&r.scratch),
        );
        let seen = Arc::new(AtomicUsize::new(0));
        let s2 = seen.clone();
        let observer: Observer = Arc::new(move |snap: &StatusSnapshot| {
            if snap.running.len() == 2 {
                s2.fetch_add(1, Ordering::SeqCst);
            }
        });
        let handle = EngineHandle::spawn(engine, Some(observer));
        handle.tick_now();
        assert_eq!(handle.snapshot().running.len(), 2);
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "observer called once for the change"
        );
        handle.tick_now();
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "and not again when nothing changed"
        );
        handle.set_paused(true);
        handle.tick_now();
        assert!(handle.snapshot().paused);
        handle.shutdown();
        assert!(host.running().is_empty(), "shutdown killed the runners");
        assert!(host.list_leftover().expect("leftover").is_empty());
        assert!(gh.runners("o/r").is_empty());
    }

    #[test]
    fn dropping_the_handle_also_stops_the_runners() {
        let mut r = rig();
        r.queue("o/r", &[1]);
        let engine = Engine::new(
            Deps {
                github: r.gh.clone(),
                host: r.host.clone(),
                clock: r.clock.clone(),
                notifier: r.notifier.clone(),
                settings: r.store.clone(),
            },
            EngineConfig::new(&r.scratch),
        );
        let handle = EngineHandle::spawn(engine, None);
        handle.tick_now();
        assert_eq!(r.host.running().len(), 1);
        drop(handle);
        assert!(r.host.running().is_empty());
    }
}
