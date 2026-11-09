//! `Notifier` via `osascript`.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use macrunnerbar_core::{Notification, Notifier, OsError};

const DEFAULT_BIN: &str = "/usr/bin/osascript";
const MAX_CHARS: usize = 200;

/// Posts a notification through `osascript`. Title and body travel as script *arguments*, never
/// spliced into script text, so job names cannot inject AppleScript.
#[derive(Debug, Clone)]
pub struct OsascriptNotifier {
    bin: PathBuf,
}

impl Default for OsascriptNotifier {
    fn default() -> Self {
        Self {
            bin: PathBuf::from(DEFAULT_BIN),
        }
    }
}

impl OsascriptNotifier {
    /// Use another `osascript` binary (tests).
    #[must_use]
    pub fn with_binary(mut self, bin: impl Into<PathBuf>) -> Self {
        self.bin = bin.into();
        self
    }
}

fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_CHARS)
        .collect()
}

impl Notifier for OsascriptNotifier {
    fn notify(&self, n: &Notification) -> Result<(), OsError> {
        let status = Command::new(&self.bin)
            .args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 1 of argv) with title (item 2 of argv)",
                "-e",
                "end run",
                "--",
            ])
            .arg(clean(&n.body))
            .arg(clean(&n.title))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| OsError::new(format!("cannot run osascript: {:?}", e.kind())))?;
        if status.success() {
            Ok(())
        } else {
            Err(OsError::new("osascript failed to post the notification"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_control_characters_and_truncates() {
        assert_eq!(clean("a\nb\u{7}c"), "abc");
        assert_eq!(clean(&"x".repeat(500)).len(), MAX_CHARS);
    }
}
