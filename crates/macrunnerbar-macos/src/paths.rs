//! Where the app keeps its files.

use std::path::{Path, PathBuf};

use macrunnerbar_core::OsError;

/// Locations under the user's home. [`Paths::under`] builds a scratch layout for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `~/Library/Application Support/MacRunnerbar`
    root: PathBuf,
    /// `~/Library/Logs/MacRunnerbar`
    logs: PathBuf,
    /// `~/Library/LaunchAgents`
    launch_agents: PathBuf,
}

impl Paths {
    /// The real layout for `home`.
    pub fn for_home(home: &Path) -> Self {
        let lib = home.join("Library");
        Self {
            root: lib.join("Application Support").join("MacRunnerbar"),
            logs: lib.join("Logs").join("MacRunnerbar"),
            launch_agents: lib.join("LaunchAgents"),
        }
    }

    /// The real layout for `$HOME`.
    pub fn from_env() -> Result<Self, OsError> {
        match std::env::var_os("HOME").map(PathBuf::from) {
            Some(h) if h.is_absolute() => Ok(Self::for_home(&h)),
            _ => Err(OsError::new("HOME is not set to an absolute path")),
        }
    }

    /// A layout entirely below `base` (tests).
    pub fn under(base: &Path) -> Self {
        Self {
            root: base.join("support"),
            logs: base.join("logs"),
            launch_agents: base.join("agents"),
        }
    }

    /// Application Support directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Log directory.
    pub fn logs_dir(&self) -> &Path {
        &self.logs
    }

    /// LaunchAgents directory.
    pub fn launch_agents_dir(&self) -> &Path {
        &self.launch_agents
    }

    /// Per-job runner directories.
    pub fn jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    /// The installed runner template.
    pub fn template_dir(&self) -> PathBuf {
        self.root.join("template")
    }

    /// Version of the installed template.
    pub fn template_version_file(&self) -> PathBuf {
        self.root.join("template.version")
    }

    /// One pid record per started runner (outside the job dir, which job code can modify).
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    /// Preferences.
    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Create the directories this app writes to (owner-only).
    pub fn ensure(&self) -> Result<(), OsError> {
        for d in [self.root.clone(), self.jobs_dir(), self.run_dir()] {
            create_private_dir(&d)?;
        }
        Ok(())
    }
}

/// `create_dir_all` with mode 0700 for directories this call creates.
pub(crate) fn create_private_dir(dir: &Path) -> Result<(), OsError> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| OsError::new(format!("cannot create {}: {:?}", dir.display(), e.kind())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_for_home() {
        let p = Paths::for_home(Path::new("/Users/x"));
        assert_eq!(
            p.root(),
            Path::new("/Users/x/Library/Application Support/MacRunnerbar")
        );
        assert_eq!(
            p.logs_dir(),
            Path::new("/Users/x/Library/Logs/MacRunnerbar")
        );
        assert_eq!(
            p.launch_agents_dir(),
            Path::new("/Users/x/Library/LaunchAgents")
        );
        assert_eq!(p.jobs_dir(), p.root().join("jobs"));
    }
}
