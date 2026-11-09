//! Shared helpers: scratch dirs, dummy runner templates, process probes.

#![allow(dead_code, unreachable_pub)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::GzEncoder;
use macrunnerbar_core::{JitConfig, Secret};

pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("mrb-macos-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn jit(value: &str) -> JitConfig {
    JitConfig {
        runner_id: 1,
        encoded: Secret::new(value.into()),
    }
}

/// Every dummy runner starts by recording its arguments, working directory and environment into files
/// in its own directory (the job dir), then runs `body`.
fn script(body: &str) -> String {
    format!("#!/bin/bash\nprintf '%s\\n' \"$@\" > args.txt\npwd > cwd.txt\nenv > env.txt\n{body}\n")
}

/// Stays up, with a grandchild and a nested subshell grandchild, until killed.
pub const STAY: &str =
    "sleep 300 &\necho $! > grandchild.pid\n( sleep 300 & echo $! > nested.pid; wait ) &\nwait";
/// Ignores SIGTERM (inherited by its children), so only SIGKILL stops it.
pub const IGNORE_TERM: &str =
    "trap '' TERM\nsleep 300 &\necho $! > grandchild.pid\nwhile true; do wait; sleep 0.1; done";
/// Exits with status 3 leaving a straggler behind.
pub const EXIT3_STRAGGLER: &str = "sleep 300 &\necho $! > grandchild.pid\nexit 3";

/// A `.tar.gz` whose top level holds an executable `run.sh` with `body`, plus a relative symlink.
pub fn template_archive(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join(format!("runner-{}.tar.gz", std::process::id()));
    let gz = GzEncoder::new(
        std::fs::File::create(&path).expect("create archive"),
        Compression::fast(),
    );
    let mut tar = tar::Builder::new(gz);
    let text = script(body);
    let mut h = tar::Header::new_gnu();
    h.set_size(text.len() as u64);
    h.set_mode(0o755);
    h.set_cksum();
    tar.append_data(&mut h, "run.sh", text.as_bytes())
        .expect("append run.sh");
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o777);
    link.set_cksum();
    tar.append_link(&mut link, "run-link.sh", "run.sh")
        .expect("append symlink");
    tar.into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip");
    path
}

pub fn alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Wait until `file` exists and holds a pid; return it.
pub fn wait_pid_file(file: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(s) = std::fs::read_to_string(file)
            && let Ok(pid) = s.trim().parse::<u32>()
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "no pid file {}", file.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn wait_dead(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        assert!(Instant::now() < deadline, "pid {pid} is still alive");
        std::thread::sleep(Duration::from_millis(20));
    }
}
