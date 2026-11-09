//! In-memory runner host.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use macrunnerbar_core::{JitConfig, OsError, ProcessState, RunnerHandle, RunnerHost};

#[derive(Debug, Default)]
struct State {
    template: Option<String>,
    next_handle: u64,
    procs: HashMap<u64, (String, ProcessState)>,
    dirs: BTreeSet<String>,
    fail_spawn: Option<OsError>,
    /// Names of runners spawned, in order.
    spawned: Vec<String>,
    /// Names of runners whose process was stopped by `kill` or `delete_dir`, in order.
    killed: Vec<String>,
}

/// Fake [`RunnerHost`]. A spawned process runs until [`finish`](Self::finish) or `kill`.
#[derive(Debug, Default)]
pub struct FakeRunnerHost {
    inner: Mutex<State>,
}

impl FakeRunnerHost {
    /// No template, no processes.
    pub fn new() -> Self {
        Self::default()
    }

    /// With a template of `version` already installed.
    pub fn with_template(version: &str) -> Self {
        let host = Self::new();
        host.lock().template = Some(version.into());
        host
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Make the process of runner `name` exit with `code`.
    pub fn finish(&self, name: &str, code: Option<i32>) {
        for (n, st) in self.lock().procs.values_mut() {
            if n == name {
                *st = ProcessState::Exited(code);
            }
        }
    }

    /// Fail the next `spawn` once.
    pub fn fail_next_spawn(&self, err: OsError) {
        self.lock().fail_spawn = Some(err);
    }

    /// Leave a runner directory behind, as a crash would.
    pub fn add_leftover(&self, name: &str) {
        self.lock().dirs.insert(name.into());
    }

    /// Names passed to `spawn`, in order.
    pub fn spawned(&self) -> Vec<String> {
        self.lock().spawned.clone()
    }

    /// Names of runners stopped by `kill` or `delete_dir` while still running, in order.
    pub fn killed(&self) -> Vec<String> {
        self.lock().killed.clone()
    }

    /// Runner names whose process is still running.
    pub fn running(&self) -> Vec<String> {
        let s = self.lock();
        let mut v: Vec<String> = s
            .procs
            .values()
            .filter(|(_, st)| *st == ProcessState::Running)
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }
}

impl RunnerHost for FakeRunnerHost {
    fn template_version(&self) -> Result<Option<String>, OsError> {
        Ok(self.lock().template.clone())
    }

    fn install_template(&self, archive: &Path, version: &str) -> Result<(), OsError> {
        if !archive.is_file() {
            return Err(OsError::new("archive missing"));
        }
        self.lock().template = Some(version.into());
        Ok(())
    }

    fn spawn(&self, name: &str, _jit: &JitConfig) -> Result<RunnerHandle, OsError> {
        let mut s = self.lock();
        if let Some(e) = s.fail_spawn.take() {
            return Err(e);
        }
        if s.template.is_none() {
            return Err(OsError::new("no runner template installed"));
        }
        s.next_handle += 1;
        let h = s.next_handle;
        s.procs.insert(h, (name.into(), ProcessState::Running));
        s.dirs.insert(name.into());
        s.spawned.push(name.into());
        Ok(RunnerHandle(h))
    }

    fn poll(&self, handle: RunnerHandle) -> Result<ProcessState, OsError> {
        self.lock()
            .procs
            .get(&handle.0)
            .map(|(_, st)| *st)
            .ok_or_else(|| OsError::new("unknown handle"))
    }

    fn kill(&self, handle: RunnerHandle) -> Result<(), OsError> {
        let mut s = self.lock();
        if let Some((name, st)) = s.procs.get_mut(&handle.0)
            && *st == ProcessState::Running
        {
            *st = ProcessState::Exited(None);
            let name = name.clone();
            s.killed.push(name);
        }
        Ok(())
    }

    fn delete_dir(&self, name: &str) -> Result<(), OsError> {
        let mut s = self.lock();
        let mut stopped = false;
        for (n, st) in s.procs.values_mut() {
            if n == name && *st == ProcessState::Running {
                *st = ProcessState::Exited(None);
                stopped = true;
            }
        }
        if stopped {
            s.killed.push(name.into());
        }
        s.dirs.remove(name);
        Ok(())
    }

    fn list_leftover(&self) -> Result<Vec<String>, OsError> {
        Ok(self.lock().dirs.iter().cloned().collect())
    }
}
