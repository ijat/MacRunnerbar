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
    /// `~/.macrunnerbar/jobs`: no whitespace, because the actions runner runs `run:` steps as
    /// `/bin/bash -e <script path>` with the path unquoted (D34).
    jobs: PathBuf,
}

impl Paths {
    /// The real layout for `home`.
    pub fn for_home(home: &Path) -> Self {
        let lib = home.join("Library");
        Self {
            root: lib.join("Application Support").join("MacRunnerbar"),
            logs: lib.join("Logs").join("MacRunnerbar"),
            launch_agents: lib.join("LaunchAgents"),
            jobs: home.join(".macrunnerbar").join("jobs"),
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
            jobs: base.join("jobs"),
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

    /// Per-job runner directories (see the `jobs` field for why this is outside Application Support).
    pub fn jobs_dir(&self) -> PathBuf {
        self.jobs.clone()
    }

    /// Where per-job directories lived before D34 (`<root>/jobs`). Only read to clean up leftovers
    /// of an older version.
    pub fn legacy_jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    /// Fails when the jobs directory path contains whitespace (a home directory with a space):
    /// every `run:` step would break (D34).
    pub fn check_jobs_dir(&self) -> Result<(), OsError> {
        let dir = self.jobs_dir();
        if dir.to_string_lossy().chars().any(char::is_whitespace) {
            return Err(OsError::new(format!(
                "the runner directory {} contains whitespace; the actions runner cannot run `run:` steps from such a path (does your home directory have a space in its path?)",
                dir.display()
            )));
        }
        Ok(())
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

    /// Create the directories this app writes to (owner-only). Fails, creating nothing, when the
    /// jobs directory path contains whitespace ([`Paths::check_jobs_dir`]).
    pub fn ensure(&self) -> Result<(), OsError> {
        self.check_jobs_dir()?;
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
        assert_eq!(p.jobs_dir(), Path::new("/Users/x/.macrunnerbar/jobs"));
        assert_eq!(p.legacy_jobs_dir(), p.root().join("jobs"));
    }

    #[test]
    fn jobs_dir_has_no_whitespace_for_a_normal_home() {
        let p = Paths::for_home(Path::new("/Users/ijat"));
        assert!(!p.jobs_dir().to_string_lossy().contains(char::is_whitespace));
        assert!(p.check_jobs_dir().is_ok());
    }

    #[test]
    fn a_home_with_a_space_is_refused_and_nothing_is_created() {
        let base = std::env::temp_dir().join(format!("mrb-paths-space-{}", std::process::id()));
        let home = base.join("Jane Doe");
        let p = Paths::for_home(&home);
        let err = p.check_jobs_dir().expect_err("whitespace must be refused");
        assert!(err.to_string().contains("whitespace"), "{err}");
        assert!(p.ensure().is_err());
        assert!(
            !home.exists(),
            "ensure created directories despite the guard"
        );
    }

    #[test]
    fn ensure_makes_the_jobs_dir_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("mrb-paths-mode-{}", std::process::id()));
        let p = Paths::for_home(&base);
        p.ensure().expect("ensure");
        for d in [base.join(".macrunnerbar"), p.jobs_dir()] {
            let mode = std::fs::metadata(&d).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", d.display());
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
