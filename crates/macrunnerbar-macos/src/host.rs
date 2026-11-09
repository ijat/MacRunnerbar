//! `RunnerHost` over real directories and process groups.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use macrunnerbar_core::{JitConfig, OsError, ProcessState, RunnerHandle, RunnerHost};

use crate::extract::extract_tar_gz;
use crate::paths::Paths;
use crate::sys;

const NAME_PREFIX: &str = "ghm-";
const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(5);
const POLL_STEP: Duration = Duration::from_millis(20);
/// Variables passed to runner processes; everything else (tokens in the app's own environment) is dropped.
const ENV_ALLOW: [&str; 8] = [
    "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE",
];
const PATH_DEFAULTS: [&str; 6] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

type EnvSource = Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

#[derive(Debug)]
struct Tracked {
    name: String,
    child: Child,
    pgid: i32,
    done: Option<ProcessState>,
}

/// Runs each runner from a fresh copy of the template, in its own process group, as the current user.
///
/// Layout under [`Paths::root`]: `template/` (+ `template.version`) and `run/<name>.pid`; per-job
/// copies (deleted afterwards) are `<name>/` under [`Paths::jobs_dir`], a space-free path outside it (D34).
/// The pid records (pid and start time, to find orphans after a crash).
pub struct ProcessRunnerHost {
    paths: Paths,
    kill_grace: Duration,
    jit_via_env: bool,
    env_source: EnvSource,
    /// Readers: `spawn` copying the template. Writer: `install_template` swapping it.
    template_lock: RwLock<()>,
    procs: Mutex<HashMap<u64, Tracked>>,
    next_handle: AtomicU64,
}

impl std::fmt::Debug for ProcessRunnerHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessRunnerHost")
            .field("root", &self.paths.root())
            .finish_non_exhaustive()
    }
}

fn io_err(what: &str, e: &std::io::Error) -> OsError {
    OsError::new(format!("{what}: {:?}", e.kind()))
}

/// Runner names we create: `ghm-` plus safe characters; never a path.
fn check_name(name: &str) -> Result<(), OsError> {
    let ok = name.starts_with(NAME_PREFIX)
        && name.len() > NAME_PREFIX.len()
        && name.len() <= 120
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok {
        Ok(())
    } else {
        Err(OsError::new(
            "runner name must be ghm- followed by [A-Za-z0-9._-]",
        ))
    }
}

fn check_version(v: &str) -> Result<(), OsError> {
    let ok = v.chars().next().is_some_and(|c| c.is_ascii_digit())
        && v.len() <= 40
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(OsError::new("unexpected runner version"))
    }
}

/// Remove a directory tree, first making directories writable if a read-only one blocks it.
fn force_remove_dir_all(dir: &Path) -> std::io::Result<()> {
    fn chmod_tree(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = fs::symlink_metadata(dir) else {
            return;
        };
        if !meta.is_dir() {
            return;
        }
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                chmod_tree(&e.path());
            }
        }
    }
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            chmod_tree(dir);
            match fs::remove_dir_all(dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        }
        Ok(()) => Ok(()),
    }
}

impl ProcessRunnerHost {
    /// A host rooted at `paths` (created on first use).
    pub fn new(paths: &Paths) -> Self {
        Self {
            paths: paths.clone(),
            kill_grace: DEFAULT_KILL_GRACE,
            jit_via_env: false,
            env_source: Box::new(|| std::env::vars().collect()),
            template_lock: RwLock::new(()),
            procs: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        }
    }

    /// How long a stopped runner gets after SIGTERM before SIGKILL (default 5 s).
    #[must_use]
    pub fn with_kill_grace(mut self, grace: Duration) -> Self {
        self.kill_grace = grace;
        self
    }

    /// Pass the JIT config as `ACTIONS_RUNNER_INPUT_JITCONFIG` instead of `--jitconfig <value>`, which
    /// keeps it out of `ps`. Off by default: the plan's `run.sh --jitconfig` form is the one to confirm first.
    #[must_use]
    pub fn with_jit_via_env(mut self, on: bool) -> Self {
        self.jit_via_env = on;
        self
    }

    /// Where the environment handed to runners is read from (tests; default: this process).
    #[must_use]
    pub fn with_env_source(
        mut self,
        f: impl Fn() -> Vec<(String, String)> + Send + Sync + 'static,
    ) -> Self {
        self.env_source = Box::new(f);
        self
    }

    fn procs(&self) -> MutexGuard<'_, HashMap<u64, Tracked>> {
        self.procs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn job_dir(&self, name: &str) -> PathBuf {
        self.paths.jobs_dir().join(name)
    }

    fn pid_file(&self, name: &str) -> PathBuf {
        self.paths.run_dir().join(format!("{name}.pid"))
    }

    /// The minimal environment for a runner: an allow-list plus a PATH that includes Homebrew.
    fn runner_env(&self) -> Vec<(String, String)> {
        let src = (self.env_source)();
        let mut out: Vec<(String, String)> = src
            .iter()
            .filter(|(k, _)| ENV_ALLOW.contains(&k.as_str()))
            .cloned()
            .collect();
        let mut dirs: Vec<String> = src
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| {
                v.split(':')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        for d in PATH_DEFAULTS {
            if !dirs.iter().any(|x| x == d) {
                dirs.push(d.to_string());
            }
        }
        out.push(("PATH".into(), dirs.join(":")));
        out
    }

    fn write_pid_file(&self, name: &str, pid: u32) -> Result<(), OsError> {
        let start = sys::start_time(pid).unwrap_or_default();
        let body = format!("{pid}\n{start}\n");
        let path = self.pid_file(name);
        let tmp = path.with_extension("pid.tmp");
        fs::write(&tmp, body).map_err(|e| io_err("cannot write pid file", &e))?;
        fs::rename(&tmp, &path).map_err(|e| io_err("cannot write pid file", &e))
    }

    fn read_pid_file(&self, name: &str) -> Option<(u32, String)> {
        let text = fs::read_to_string(self.pid_file(name)).ok()?;
        let mut lines = text.lines();
        let pid = lines.next()?.trim().parse::<u32>().ok()?;
        let start = lines.next()?.trim().to_string();
        (!start.is_empty()).then_some((pid, start))
    }

    /// Stop a tracked process: SIGTERM to its group, wait up to the grace period for the leader, then
    /// SIGKILL the whole group (sweeping stragglers) and reap. No-op once finished.
    fn stop_tracked(&self, handle: u64) {
        let (pid, pgid) = {
            let procs = self.procs();
            match procs.get(&handle) {
                Some(t) if t.done.is_none() => (t.child.id(), t.pgid),
                _ => return,
            }
        };
        let _ = sys::killpg(pgid, sys::SIGTERM);
        let deadline = Instant::now() + self.kill_grace;
        while !sys::leader_exited(pid) && Instant::now() < deadline {
            std::thread::sleep(POLL_STEP);
        }
        if let Some(t) = self.procs().get_mut(&handle) {
            Self::finish(t);
        }
    }

    /// The leader is gone or must go: SIGKILL its group (stragglers included), then reap it.
    fn finish(t: &mut Tracked) -> ProcessState {
        if let Some(s) = t.done {
            return s;
        }
        // The leader is still unreaped (a zombie at worst), so `pgid` cannot have been recycled.
        let _ = sys::killpg(t.pgid, sys::SIGKILL);
        let state = match t.child.wait() {
            Ok(status) => ProcessState::Exited(status.code()),
            Err(_) => ProcessState::Exited(None),
        };
        t.done = Some(state);
        state
    }

    /// Stop a runner this host did not start in this run (found through its pid file) if the recorded
    /// pid still has the recorded start time. Never signals a recycled pid.
    fn stop_recorded(&self, name: &str) {
        let Some((pid, start)) = self.read_pid_file(name) else {
            return;
        };
        if sys::start_time(pid).as_deref() != Some(start.as_str()) {
            return;
        }
        let Ok(pgid) = i32::try_from(pid) else { return };
        let _ = sys::killpg(pgid, sys::SIGTERM);
        let deadline = Instant::now() + self.kill_grace;
        while sys::start_time(pid).as_deref() == Some(start.as_str()) && Instant::now() < deadline {
            std::thread::sleep(POLL_STEP);
        }
        let _ = sys::killpg(pgid, sys::SIGKILL);
    }

    fn clone_template(&self, dest: &Path) -> Result<(), OsError> {
        let src = self.paths.template_dir();
        // APFS clone first (instant, copy-on-write); fall back to a plain recursive copy.
        for flags in ["-cR", "-R"] {
            let status = Command::new("/bin/cp")
                .arg(flags)
                .arg(&src)
                .arg(dest)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| io_err("cannot run cp", &e))?;
            if status.success() {
                return Ok(());
            }
            let _ = force_remove_dir_all(dest);
        }
        Err(OsError::new("cannot copy the runner template"))
    }
}

impl RunnerHost for ProcessRunnerHost {
    fn template_version(&self) -> Result<Option<String>, OsError> {
        let _g = self
            .template_lock
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if !self.paths.template_dir().join("run.sh").is_file() {
            return Ok(None);
        }
        match fs::read_to_string(self.paths.template_version_file()) {
            Ok(s) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err("cannot read template version", &e)),
        }
    }

    fn install_template(&self, archive: &Path, version: &str) -> Result<(), OsError> {
        check_version(version)?;
        self.paths.ensure()?;
        let staging = self.paths.root().join("template.staging");
        force_remove_dir_all(&staging).map_err(|e| io_err("cannot clear staging", &e))?;
        fs::create_dir(&staging).map_err(|e| io_err("cannot create staging", &e))?;
        let built = extract_tar_gz(archive, &staging).and_then(|()| {
            if staging.join("run.sh").is_file() {
                Ok(())
            } else {
                Err(OsError::new(
                    "runner archive has no run.sh at its top level",
                ))
            }
        });
        if let Err(e) = built {
            let _ = force_remove_dir_all(&staging);
            return Err(e);
        }
        let _g = self
            .template_lock
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let template = self.paths.template_dir();
        let old = self.paths.root().join("template.old");
        force_remove_dir_all(&old).map_err(|e| io_err("cannot clear old template", &e))?;
        if template.exists() {
            fs::rename(&template, &old).map_err(|e| io_err("cannot move the old template", &e))?;
        }
        if let Err(e) = fs::rename(&staging, &template) {
            if old.exists() {
                let _ = fs::rename(&old, &template); // put the previous template back
            }
            return Err(io_err("cannot install the template", &e));
        }
        let ver_tmp = self.paths.root().join("template.version.tmp");
        fs::write(&ver_tmp, format!("{version}\n"))
            .and_then(|()| fs::rename(&ver_tmp, self.paths.template_version_file()))
            .map_err(|e| io_err("cannot record the template version", &e))?;
        let _ = force_remove_dir_all(&old);
        Ok(())
    }

    fn spawn(&self, name: &str, jit: &JitConfig) -> Result<RunnerHandle, OsError> {
        use std::os::unix::process::CommandExt;
        check_name(name)?;
        self.paths.ensure()?;
        let job_dir = self.job_dir(name);
        {
            let _g = self
                .template_lock
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            if !self.paths.template_dir().join("run.sh").is_file() {
                return Err(OsError::new("no runner template installed"));
            }
            if job_dir.exists() {
                return Err(OsError::new(
                    "a job directory with this name already exists",
                ));
            }
            self.clone_template(&job_dir)?;
        }
        let mut cmd = Command::new(job_dir.join("run.sh"));
        cmd.current_dir(&job_dir)
            .env_clear()
            .envs(self.runner_env())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        // After `env_clear`, which would otherwise drop it.
        if self.jit_via_env {
            cmd.env("ACTIONS_RUNNER_INPUT_JITCONFIG", jit.encoded.expose());
        } else {
            cmd.arg("--jitconfig").arg(jit.encoded.expose());
        }
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = force_remove_dir_all(&job_dir);
                return Err(io_err("cannot start run.sh", &e));
            }
        };
        let pid = child.id();
        let pgid = i32::try_from(pid).unwrap_or(0);
        let handle = self.next_handle.fetch_add(1, Ordering::SeqCst);
        let mut tracked = Tracked {
            name: name.to_string(),
            child,
            pgid,
            done: None,
        };
        if let Err(e) = self.write_pid_file(name, pid) {
            Self::finish(&mut tracked);
            let _ = force_remove_dir_all(&job_dir);
            return Err(e);
        }
        self.procs().insert(handle, tracked);
        Ok(RunnerHandle(handle))
    }

    fn poll(&self, handle: RunnerHandle) -> Result<ProcessState, OsError> {
        let mut procs = self.procs();
        let t = procs
            .get_mut(&handle.0)
            .ok_or_else(|| OsError::new("unknown runner handle"))?;
        if let Some(s) = t.done {
            return Ok(s);
        }
        if sys::leader_exited(t.child.id()) {
            Ok(Self::finish(t))
        } else {
            Ok(ProcessState::Running)
        }
    }

    fn kill(&self, handle: RunnerHandle) -> Result<(), OsError> {
        self.stop_tracked(handle.0);
        Ok(())
    }

    fn delete_dir(&self, name: &str) -> Result<(), OsError> {
        check_name(name)?;
        let handles: Vec<u64> = self
            .procs()
            .iter()
            .filter(|(_, t)| t.name == name)
            .map(|(h, _)| *h)
            .collect();
        for h in &handles {
            self.stop_tracked(*h);
        }
        if handles.is_empty() {
            self.stop_recorded(name);
        }
        {
            let mut procs = self.procs();
            for h in handles {
                procs.remove(&h);
            }
        }
        // The current location, and the pre-D34 one under Application Support (a leftover of an older
        // version; absent otherwise).
        for dir in [self.job_dir(name), self.paths.legacy_jobs_dir().join(name)] {
            let mut removed = force_remove_dir_all(&dir);
            for _ in 0..3 {
                if removed.is_ok() {
                    break;
                }
                // A dying process may still be writing into the directory.
                std::thread::sleep(Duration::from_millis(100));
                removed = force_remove_dir_all(&dir);
            }
            removed.map_err(|e| io_err("cannot remove the job directory", &e))?;
        }
        match fs::remove_file(self.pid_file(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(io_err("cannot remove pid file", &e))
            }
            _ => Ok(()),
        }
    }

    fn list_leftover(&self) -> Result<Vec<String>, OsError> {
        let mut names: Vec<String> = Vec::new();
        for (dir, strip) in [
            (self.paths.jobs_dir(), ""),
            (self.paths.legacy_jobs_dir(), ""),
            (self.paths.run_dir(), ".pid"),
        ] {
            let rd = match fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_err("cannot list runner directories", &e)),
            };
            for entry in rd.flatten() {
                let file = entry.file_name().to_string_lossy().to_string();
                let name = file.strip_suffix(strip).unwrap_or(&file);
                if (strip.is_empty() || file.ends_with(strip)) && check_name(name).is_ok() {
                    names.push(name.to_string());
                }
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }
}
