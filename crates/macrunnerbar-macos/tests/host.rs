//! `ProcessRunnerHost` against dummy runner scripts in a scratch app-support directory.

mod support;

use std::time::Duration;

use macrunnerbar_core::{ProcessState, RunnerHost};
use macrunnerbar_macos::{Paths, ProcessRunnerHost};
use macrunnerbar_testkit::contract;
use support::{
    EXIT3_STRAGGLER, IGNORE_TERM, STAY, Scratch, alive, jit, template_archive, wait_dead,
    wait_pid_file,
};

const SENTINEL_ENV: &str = "SENTINEL_PAT_VALUE_0123456789";

fn host_with(body: &str, scratch: &Scratch) -> (ProcessRunnerHost, Paths) {
    let paths = Paths::under(scratch.path());
    let host = ProcessRunnerHost::new(&paths)
        .with_kill_grace(Duration::from_secs(2))
        .with_env_source(|| {
            vec![
                ("HOME".into(), "/Users/test".into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("GITHUB_MASTER_CLASSIC_PAT".into(), SENTINEL_ENV.into()),
                ("AWS_SECRET_ACCESS_KEY".into(), SENTINEL_ENV.into()),
            ]
        });
    let archive = template_archive(scratch.path(), body);
    host.install_template(&archive, "9.9.9")
        .expect("install template");
    (host, paths)
}

fn read(path: std::path::PathBuf) -> String {
    std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("cannot read {}", path.display()))
}

#[test]
fn passes_the_runner_host_contract_suites() {
    let scratch = Scratch::new("contract");
    let (host, _) = host_with(STAY, &scratch);
    contract::runner_host_contract(&host, &jit("JIT-CONTRACT"));
    let fresh = ProcessRunnerHost::new(&Paths::under(&scratch.path().join("other")));
    contract::runner_host_template_contract(&fresh, &template_archive(scratch.path(), STAY));
}

#[test]
fn template_install_records_version_and_copy_keeps_exec_bit_and_symlink() {
    let scratch = Scratch::new("install");
    let (host, paths) = host_with(STAY, &scratch);
    assert_eq!(
        host.template_version().expect("version").as_deref(),
        Some("9.9.9")
    );
    let h = host.spawn("ghm-o-r-aaaaaa", &jit("X")).expect("spawn");
    let job = paths.jobs_dir().join("ghm-o-r-aaaaaa");
    assert!(job.join("run.sh").is_file());
    assert!(
        job.join("run-link.sh")
            .symlink_metadata()
            .expect("link")
            .is_symlink()
    );
    use std::os::unix::fs::PermissionsExt;
    assert_ne!(
        job.join("run.sh")
            .metadata()
            .expect("meta")
            .permissions()
            .mode()
            & 0o111,
        0
    );
    host.kill(h).expect("kill");
    host.delete_dir("ghm-o-r-aaaaaa").expect("delete");
}

#[test]
fn spawn_without_a_template_fails() {
    let scratch = Scratch::new("notemplate");
    let host = ProcessRunnerHost::new(&Paths::under(scratch.path()));
    assert_eq!(host.template_version().expect("version"), None);
    assert!(host.spawn("ghm-o-r-aaaaaa", &jit("X")).is_err());
}

#[test]
fn runner_gets_jitconfig_argument_cwd_in_its_job_dir_and_a_scrubbed_environment() {
    let scratch = Scratch::new("args");
    let (host, paths) = host_with(STAY, &scratch);
    let h = host
        .spawn("ghm-o-r-bbbbbb", &jit("ENCODED-JIT"))
        .expect("spawn");
    let job = paths.jobs_dir().join("ghm-o-r-bbbbbb");
    wait_pid_file(&job.join("nested.pid"));
    assert_eq!(read(job.join("args.txt")), "--jitconfig\nENCODED-JIT\n");
    let cwd = read(job.join("cwd.txt"));
    assert!(cwd.trim().ends_with("jobs/ghm-o-r-bbbbbb"), "cwd was {cwd}");
    // Only key names are asserted on; the environment itself is never printed.
    let env = read(job.join("env.txt"));
    assert!(env.contains("HOME=/Users/test"));
    assert!(env.contains("PATH=/usr/bin:/bin:/opt/homebrew/bin"));
    assert!(
        !env.contains(SENTINEL_ENV),
        "a non-allow-listed variable value reached the runner"
    );
    assert!(
        !env.contains("GITHUB_MASTER_CLASSIC_PAT"),
        "GITHUB_MASTER_CLASSIC_PAT reached the runner"
    );
    assert!(
        !env.contains("AWS_SECRET_ACCESS_KEY"),
        "AWS_SECRET_ACCESS_KEY reached the runner"
    );
    assert!(!env.contains("ACTIONS_RUNNER_INPUT_JITCONFIG"));
    host.kill(h).expect("kill");
    host.delete_dir("ghm-o-r-bbbbbb").expect("delete");
}

#[test]
fn jit_via_env_keeps_the_config_out_of_the_argument_list() {
    let scratch = Scratch::new("env");
    let paths = Paths::under(scratch.path());
    let host = ProcessRunnerHost::new(&paths)
        .with_kill_grace(Duration::from_secs(2))
        .with_jit_via_env(true)
        .with_env_source(Vec::new);
    host.install_template(&template_archive(scratch.path(), STAY), "1.0.0")
        .expect("install");
    let h = host
        .spawn("ghm-o-r-cccccc", &jit("ENV-JIT"))
        .expect("spawn");
    let job = paths.jobs_dir().join("ghm-o-r-cccccc");
    wait_pid_file(&job.join("nested.pid"));
    assert_eq!(read(job.join("args.txt")), "\n", "no arguments");
    assert!(read(job.join("env.txt")).contains("ACTIONS_RUNNER_INPUT_JITCONFIG=ENV-JIT"));
    host.kill(h).expect("kill");
    host.delete_dir("ghm-o-r-cccccc").expect("delete");
}

#[test]
fn kill_stops_the_runner_and_every_process_in_its_group() {
    let scratch = Scratch::new("killgroup");
    let (host, paths) = host_with(STAY, &scratch);
    let h = host.spawn("ghm-o-r-dddddd", &jit("X")).expect("spawn");
    let job = paths.jobs_dir().join("ghm-o-r-dddddd");
    let child = wait_pid_file(&job.join("grandchild.pid"));
    let nested = wait_pid_file(&job.join("nested.pid"));
    assert!(alive(child) && alive(nested));
    assert_eq!(host.poll(h).expect("poll"), ProcessState::Running);
    host.kill(h).expect("kill");
    wait_dead(child);
    wait_dead(nested);
    assert!(matches!(
        host.poll(h).expect("poll after kill"),
        ProcessState::Exited(_)
    ));
    host.delete_dir("ghm-o-r-dddddd").expect("delete");
}

#[test]
fn kill_escalates_to_sigkill_when_the_runner_ignores_sigterm() {
    let scratch = Scratch::new("sigkill");
    let (host, paths) = host_with(IGNORE_TERM, &scratch);
    let h = host.spawn("ghm-o-r-eeeeee", &jit("X")).expect("spawn");
    let child = wait_pid_file(
        &paths
            .jobs_dir()
            .join("ghm-o-r-eeeeee")
            .join("grandchild.pid"),
    );
    let started = std::time::Instant::now();
    host.kill(h).expect("kill");
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "SIGTERM should have been given its grace period"
    );
    wait_dead(child);
    assert!(
        matches!(host.poll(h).expect("poll"), ProcessState::Exited(None)),
        "killed by a signal has no exit code"
    );
    host.delete_dir("ghm-o-r-eeeeee").expect("delete");
}

#[test]
fn natural_exit_reports_the_code_and_sweeps_stragglers() {
    let scratch = Scratch::new("exit");
    let (host, paths) = host_with(EXIT3_STRAGGLER, &scratch);
    let h = host.spawn("ghm-o-r-ffffff", &jit("X")).expect("spawn");
    let job = paths.jobs_dir().join("ghm-o-r-ffffff");
    let straggler = wait_pid_file(&job.join("grandchild.pid"));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let state = loop {
        match host.poll(h).expect("poll") {
            ProcessState::Running => {
                assert!(std::time::Instant::now() < deadline, "runner never exited");
                std::thread::sleep(Duration::from_millis(20));
            }
            s => break s,
        }
    };
    assert_eq!(state, ProcessState::Exited(Some(3)));
    wait_dead(straggler);
    host.delete_dir("ghm-o-r-ffffff").expect("delete");
    assert!(!job.exists());
}

#[test]
fn delete_dir_removes_dir_and_pid_record_and_stops_a_running_runner() {
    let scratch = Scratch::new("delete");
    let (host, paths) = host_with(STAY, &scratch);
    host.spawn("ghm-o-r-111111", &jit("X")).expect("spawn");
    let job = paths.jobs_dir().join("ghm-o-r-111111");
    let child = wait_pid_file(&job.join("grandchild.pid"));
    assert!(paths.run_dir().join("ghm-o-r-111111.pid").is_file());
    host.delete_dir("ghm-o-r-111111")
        .expect("delete while running");
    wait_dead(child);
    assert!(!job.exists());
    assert!(!paths.run_dir().join("ghm-o-r-111111.pid").exists());
    assert!(host.list_leftover().expect("leftover").is_empty());
    host.delete_dir("ghm-o-r-111111").expect("idempotent");
}

#[test]
fn each_job_gets_a_fresh_directory_and_a_name_cannot_be_reused() {
    let scratch = Scratch::new("fresh");
    let (host, paths) = host_with(STAY, &scratch);
    let a = host.spawn("ghm-o-r-aaaaa1", &jit("X")).expect("a");
    let b = host.spawn("ghm-o-r-aaaaa2", &jit("X")).expect("b");
    assert!(
        host.spawn("ghm-o-r-aaaaa1", &jit("X")).is_err(),
        "dir exists"
    );
    wait_pid_file(
        &paths
            .jobs_dir()
            .join("ghm-o-r-aaaaa1")
            .join("grandchild.pid"),
    );
    std::fs::write(
        paths.jobs_dir().join("ghm-o-r-aaaaa1").join("mark.txt"),
        "a",
    )
    .expect("mark");
    assert!(
        !paths
            .jobs_dir()
            .join("ghm-o-r-aaaaa2")
            .join("mark.txt")
            .exists()
    );
    assert!(!paths.template_dir().join("mark.txt").exists());
    host.kill(a).expect("kill a");
    host.kill(b).expect("kill b");
    host.delete_dir("ghm-o-r-aaaaa1").expect("delete a");
    host.delete_dir("ghm-o-r-aaaaa2").expect("delete b");
}

#[test]
fn names_that_are_not_ghm_or_contain_path_characters_are_rejected_and_touch_nothing() {
    let scratch = Scratch::new("names");
    let (host, paths) = host_with(STAY, &scratch);
    let outside = scratch.path().join("precious");
    std::fs::create_dir_all(&outside).expect("outside");
    for bad in [
        "gha-x",
        "ghm-",
        "../precious",
        "ghm-../../precious",
        "ghm-a/b",
        "",
        "ghm-a b",
    ] {
        assert!(host.spawn(bad, &jit("X")).is_err(), "spawn {bad:?}");
        assert!(host.delete_dir(bad).is_err(), "delete {bad:?}");
    }
    assert!(outside.is_dir());
    assert!(host.list_leftover().expect("leftover").is_empty());
    // A foreign directory in jobs/ (not ghm-) is neither listed nor deleted.
    std::fs::create_dir_all(paths.jobs_dir().join("gha-foreign")).expect("foreign");
    assert!(host.list_leftover().expect("leftover").is_empty());
    assert!(paths.jobs_dir().join("gha-foreign").is_dir());
}

#[test]
fn crash_leftovers_are_found_and_cleaned_by_a_new_host() {
    let scratch = Scratch::new("orphan");
    let (old_host, paths) = host_with(STAY, &scratch);
    old_host.spawn("ghm-o-r-crash1", &jit("X")).expect("spawn");
    let job = paths.jobs_dir().join("ghm-o-r-crash1");
    let child = wait_pid_file(&job.join("grandchild.pid"));
    // The app "crashes": the host (and its bookkeeping) is gone; the runner keeps running.
    drop(old_host);
    assert!(alive(child));

    let new_host = ProcessRunnerHost::new(&paths).with_kill_grace(Duration::from_secs(2));
    assert_eq!(
        new_host.list_leftover().expect("leftover"),
        vec!["ghm-o-r-crash1"]
    );
    for name in new_host.list_leftover().expect("leftover") {
        new_host.delete_dir(&name).expect("cleanup");
    }
    wait_dead(child);
    assert!(!job.exists());
    assert!(new_host.list_leftover().expect("leftover").is_empty());
}

#[test]
fn a_recycled_pid_in_a_pid_file_is_not_signalled() {
    use std::os::unix::process::CommandExt;
    let scratch = Scratch::new("recycled");
    let paths = Paths::under(scratch.path());
    paths.ensure().expect("ensure");
    // An innocent process in its own group, standing in for whatever now owns the recorded pid.
    let mut innocent = std::process::Command::new("/bin/sleep")
        .arg("300")
        .process_group(0)
        .spawn()
        .expect("spawn sleep");
    let pid = innocent.id();
    std::fs::write(
        paths.run_dir().join("ghm-o-r-reuse1.pid"),
        format!("{pid}\nMon Jan  1 00:00:00 1990\n"),
    )
    .expect("pid file");
    std::fs::create_dir_all(paths.jobs_dir().join("ghm-o-r-reuse1")).expect("job dir");
    let host = ProcessRunnerHost::new(&paths).with_kill_grace(Duration::from_millis(300));
    host.delete_dir("ghm-o-r-reuse1").expect("cleanup");
    assert!(
        innocent.try_wait().expect("try_wait").is_none(),
        "an unrelated process was killed"
    );
    assert!(!paths.jobs_dir().join("ghm-o-r-reuse1").exists());
    innocent.kill().expect("stop the stand-in");
    innocent.wait().expect("wait");
}

#[test]
fn install_replaces_the_template_and_rejects_archives_without_run_sh() {
    let scratch = Scratch::new("reinstall");
    let (host, paths) = host_with(STAY, &scratch);
    // An archive with no run.sh must fail and leave the old template and version intact.
    let bad = scratch.path().join("empty.tar.gz");
    {
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&bad).expect("create"),
            flate2::Compression::fast(),
        );
        tar::Builder::new(gz)
            .into_inner()
            .expect("tar")
            .finish()
            .expect("gz");
    }
    assert!(host.install_template(&bad, "10.0.0").is_err());
    assert_eq!(
        host.template_version().expect("version").as_deref(),
        Some("9.9.9")
    );
    assert!(paths.template_dir().join("run.sh").is_file());
    assert!(!paths.root().join("template.staging").exists());
    // A good one replaces it.
    host.install_template(&template_archive(scratch.path(), EXIT3_STRAGGLER), "10.0.1")
        .expect("reinstall");
    assert_eq!(
        host.template_version().expect("version").as_deref(),
        Some("10.0.1")
    );
    assert!(
        host.install_template(&bad, "../evil").is_err(),
        "bad version string"
    );
    assert!(!paths.root().join("template.old").exists());
}

type GzBuilder = tar::Builder<flate2::write::GzEncoder<std::fs::File>>;

fn build_archive(path: &std::path::Path, f: &dyn Fn(&mut GzBuilder)) {
    let gz = flate2::write::GzEncoder::new(
        std::fs::File::create(path).expect("create"),
        flate2::Compression::fast(),
    );
    let mut b = tar::Builder::new(gz);
    f(&mut b);
    b.into_inner().expect("tar").finish().expect("gz");
}

/// A regular file entry whose raw name bypasses the builder's `..` check.
fn raw_file(b: &mut GzBuilder, path: &str) {
    let mut h = tar::Header::new_gnu();
    h.set_size(4);
    h.set_mode(0o644);
    let bytes = path.as_bytes();
    h.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
    h.set_cksum();
    b.append(&h, &b"evil"[..]).expect("append");
}

fn raw_link(b: &mut GzBuilder, kind: tar::EntryType, name: &str, target: &str) {
    let mut l = tar::Header::new_gnu();
    l.set_entry_type(kind);
    l.set_size(0);
    l.set_cksum();
    b.append_link(&mut l, name, target).expect("link entry");
}

#[test]
fn extraction_refuses_path_traversal_symlink_write_through_and_hard_links() {
    let scratch = Scratch::new("traversal");
    let paths = Paths::under(&scratch.path().join("app"));
    let host = ProcessRunnerHost::new(&paths);
    let outside = scratch.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    let out = |n: &str| scratch.path().join(n);

    build_archive(&out("dotdot.tar.gz"), &|b| {
        raw_file(b, "run.sh");
        raw_file(b, "../pwned.txt");
    });
    assert!(
        host.install_template(&out("dotdot.tar.gz"), "1.0.0")
            .is_err()
    );
    assert!(!scratch.path().join("pwned.txt").exists());
    assert!(!scratch.path().join("app").join("pwned.txt").exists());

    let target = outside.to_string_lossy().to_string();
    build_archive(&out("through.tar.gz"), &|b| {
        raw_file(b, "run.sh");
        raw_link(b, tar::EntryType::Symlink, "link", &target);
        raw_file(b, "link/pwned.txt");
    });
    assert!(
        host.install_template(&out("through.tar.gz"), "1.0.0")
            .is_err(),
        "absolute symlink"
    );
    assert!(!outside.join("pwned.txt").exists());

    build_archive(&out("relative.tar.gz"), &|b| {
        raw_file(b, "run.sh");
        raw_link(b, tar::EntryType::Symlink, "up", "../../..");
        raw_file(b, "up/pwned.txt");
    });
    assert!(
        host.install_template(&out("relative.tar.gz"), "1.0.0")
            .is_err(),
        "escaping symlink"
    );

    build_archive(&out("hardlink.tar.gz"), &|b| {
        raw_file(b, "run.sh");
        raw_link(b, tar::EntryType::Link, "hl", "run.sh");
    });
    assert!(
        host.install_template(&out("hardlink.tar.gz"), "1.0.0")
            .is_err(),
        "hard links"
    );
    assert_eq!(
        host.template_version().expect("version"),
        None,
        "nothing was installed"
    );
}
