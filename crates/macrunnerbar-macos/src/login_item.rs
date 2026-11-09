//! Start at login: a LaunchAgent plist in `~/Library/LaunchAgents`.
//!
//! launchd loads that directory at the next login, so enabling only writes the plist and disabling
//! only removes it. `launchctl bootstrap` is deliberately not called: with `RunAtLoad` it would start a
//! second copy of the app now, and `bootout` would terminate the running app when it is the one launchd
//! started.

use std::fs;
use std::path::{Path, PathBuf};

use macrunnerbar_core::{LoginItem, OsError};

const DEFAULT_LABEL: &str = "my.ijat.macrunnerbar";

/// What the plist says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAgentSpec {
    /// `Label`.
    pub label: String,
    /// `ProgramArguments`.
    pub program_arguments: Vec<String>,
    /// `RunAtLoad`.
    pub run_at_load: bool,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

impl LaunchAgentSpec {
    /// The plist text.
    pub fn render(&self) -> String {
        let args: String = self
            .program_arguments
            .iter()
            .map(|a| format!("\t\t<string>{}</string>\n", escape(a)))
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
\t<key>Label</key>\n\t<string>{}</string>\n\
\t<key>ProgramArguments</key>\n\t<array>\n{args}\t</array>\n\
\t<key>RunAtLoad</key>\n\t<{}/>\n\
\t<key>KeepAlive</key>\n\t<false/>\n\
\t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
\t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n\
</dict>\n</plist>\n",
            escape(&self.label),
            if self.run_at_load { "true" } else { "false" },
        )
    }

    /// Read back a plist written by [`render`](Self::render). Strict: anything else is an error.
    pub fn parse(text: &str) -> Result<Self, OsError> {
        let bad = || OsError::new("not a MacRunnerbar LaunchAgent plist");
        let after_key = |key: &str| -> Result<&str, OsError> {
            let marker = format!("<key>{key}</key>");
            text.find(&marker)
                .map(|i| text[i + marker.len()..].trim_start())
                .ok_or_else(bad)
        };
        let string_at = |rest: &str| -> Result<String, OsError> {
            let body = rest.strip_prefix("<string>").ok_or_else(bad)?;
            let end = body.find("</string>").ok_or_else(bad)?;
            Ok(unescape(&body[..end]))
        };
        let label = string_at(after_key("Label")?)?;
        let arr = after_key("ProgramArguments")?
            .strip_prefix("<array>")
            .ok_or_else(bad)?;
        let arr = &arr[..arr.find("</array>").ok_or_else(bad)?];
        let mut program_arguments = Vec::new();
        let mut rest = arr.trim_start();
        while !rest.is_empty() {
            program_arguments.push(string_at(rest)?);
            let end = rest.find("</string>").ok_or_else(bad)? + "</string>".len();
            rest = rest[end..].trim_start();
        }
        let run_at_load = {
            let v = after_key("RunAtLoad")?;
            if v.starts_with("<true/>") {
                true
            } else if v.starts_with("<false/>") {
                false
            } else {
                return Err(bad());
            }
        };
        Ok(Self {
            label,
            program_arguments,
            run_at_load,
        })
    }
}

/// [`LoginItem`] backed by a LaunchAgent plist.
#[derive(Debug, Clone)]
pub struct LaunchAgent {
    dir: PathBuf,
    label: String,
    executable: PathBuf,
}

impl LaunchAgent {
    /// An agent in `launch_agents_dir` that starts `executable` (an absolute path) at login.
    pub fn new(launch_agents_dir: &Path, executable: &Path) -> Result<Self, OsError> {
        if !executable.is_absolute() {
            return Err(OsError::new("the executable path must be absolute"));
        }
        Ok(Self {
            dir: launch_agents_dir.to_path_buf(),
            label: DEFAULT_LABEL.into(),
            executable: executable.to_path_buf(),
        })
    }

    /// Where the plist lives.
    pub fn plist_path(&self) -> PathBuf {
        self.dir.join(format!("{}.plist", self.label))
    }

    fn spec(&self) -> LaunchAgentSpec {
        LaunchAgentSpec {
            label: self.label.clone(),
            program_arguments: vec![self.executable.to_string_lossy().to_string()],
            run_at_load: true,
        }
    }

    /// The installed plist, if any.
    pub fn read(&self) -> Result<Option<LaunchAgentSpec>, OsError> {
        match fs::read_to_string(self.plist_path()) {
            Ok(text) => LaunchAgentSpec::parse(&text).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(OsError::new(format!(
                "cannot read the LaunchAgent plist: {:?}",
                e.kind()
            ))),
        }
    }
}

impl LoginItem for LaunchAgent {
    fn is_enabled(&self) -> Result<bool, OsError> {
        Ok(self
            .read()?
            .is_some_and(|s| s.label == self.label && s.run_at_load))
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), OsError> {
        let path = self.plist_path();
        if enabled {
            fs::create_dir_all(&self.dir).map_err(|e| {
                OsError::new(format!(
                    "cannot create {}: {:?}",
                    self.dir.display(),
                    e.kind()
                ))
            })?;
            let tmp = path.with_extension("plist.tmp");
            fs::write(&tmp, self.spec().render())
                .and_then(|()| fs::rename(&tmp, &path))
                .map_err(|e| {
                    OsError::new(format!(
                        "cannot write the LaunchAgent plist: {:?}",
                        e.kind()
                    ))
                })
        } else {
            match fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(OsError::new(format!(
                    "cannot remove the LaunchAgent plist: {:?}",
                    e.kind()
                ))),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_parse_round_trip_with_special_characters() {
        let spec = LaunchAgentSpec {
            label: "my.ijat.macrunnerbar".into(),
            program_arguments: vec![
                "/Applications/Mac & <Runner> \"bar\".app/Contents/MacOS/x".into(),
                "--flag".into(),
            ],
            run_at_load: true,
        };
        assert_eq!(LaunchAgentSpec::parse(&spec.render()).expect("parse"), spec);
    }

    #[test]
    fn parse_rejects_foreign_text() {
        assert!(LaunchAgentSpec::parse("hello").is_err());
        assert!(
            LaunchAgentSpec::parse(
                "<plist><dict><key>Label</key><integer>1</integer></dict></plist>"
            )
            .is_err()
        );
    }
}
