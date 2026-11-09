//! Rotating file log under `~/Library/Logs/MacRunnerbar/` with a redacting writer.
//!
//! Every formatted event passes through [`redact`] before it reaches the file: GitHub token shapes,
//! `Bearer` credentials and any secret registered with the [`Redactor`] become `[redacted]`.

use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use macrunnerbar_core::{OsError, Secret};
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::MakeWriter;

const REDACTED: &str = "[redacted]";
const TOKEN_PREFIXES: [&str; 6] = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
const MIN_SECRET_LEN: usize = 6;
const KEEP_FILES: usize = 14;

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Replace secrets in one line or a whole buffer.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        if s.len() >= MIN_SECRET_LEN {
            out = out.replace(s.as_str(), REDACTED);
        }
    }
    // GitHub token shapes and `Bearer <credential>`.
    let mut result = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while !rest.is_empty() {
        let next = TOKEN_PREFIXES
            .iter()
            .filter_map(|p| rest.find(p).map(|i| (i, p.len(), false)))
            .chain(rest.find("Bearer ").map(|i| (i, "Bearer ".len(), true)))
            .min_by_key(|(i, _, _)| *i);
        let Some((i, plen, bearer)) = next else {
            result.push_str(rest);
            break;
        };
        result.push_str(&rest[..i]);
        let tail = &rest[i + plen..];
        let run = tail
            .find(|c: char| !(is_token_char(c) || (bearer && matches!(c, '-' | '.' | '='))))
            .unwrap_or(tail.len());
        if bearer {
            result.push_str("Bearer ");
        }
        result.push_str(REDACTED);
        rest = &tail[run..];
    }
    result
}

/// Registry of exact secret values to scrub from the log (the token typed into Preferences, a JIT
/// config). Cheap to clone and share.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Arc<RwLock<Vec<String>>>,
}

impl Redactor {
    /// Scrub this value from every future log line.
    pub fn add(&self, secret: &Secret<String>) {
        let mut v = self.secrets.write().unwrap_or_else(PoisonError::into_inner);
        let s = secret.expose();
        if s.len() >= MIN_SECRET_LEN && !v.contains(s) {
            v.push(s.clone());
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.secrets
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[derive(Debug, Clone)]
struct RedactingMakeWriter {
    sink: Arc<Mutex<RollingFileAppender>>,
    redactor: Redactor,
}

/// Buffers one event and writes it redacted when flushed or dropped.
#[derive(Debug)]
struct RedactingWriter {
    buf: Vec<u8>,
    sink: Arc<Mutex<RollingFileAppender>>,
    redactor: Redactor,
}

impl RedactingWriter {
    fn emit(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let clean = redact(
            &String::from_utf8_lossy(&self.buf),
            &self.redactor.snapshot(),
        );
        self.buf.clear();
        let mut sink = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        sink.write_all(clean.as_bytes())
    }
}

impl Write for RedactingWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit()
    }
}

impl Drop for RedactingWriter {
    fn drop(&mut self) {
        let _ = self.emit();
    }
}

impl<'a> MakeWriter<'a> for RedactingMakeWriter {
    type Writer = RedactingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter {
            buf: Vec::new(),
            sink: self.sink.clone(),
            redactor: self.redactor.clone(),
        }
    }
}

/// A subscriber that writes daily-rotated `macrunnerbar.<date>.log` files in `dir` (14 kept), plus the
/// [`Redactor`] that scrubs registered secrets. Not installed; see [`init`].
pub fn subscriber(
    dir: &Path,
    level: Level,
) -> Result<(impl tracing::Subscriber + Send + Sync + 'static, Redactor), OsError> {
    crate::paths::create_private_dir(dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("macrunnerbar")
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(dir)
        .map_err(|_| OsError::new("cannot open the log file"))?;
    let redactor = Redactor::default();
    let writer = RedactingMakeWriter {
        sink: Arc::new(Mutex::new(appender)),
        redactor: redactor.clone(),
    };
    let sub = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(level)
        .finish();
    Ok((sub, redactor))
}

/// Install the global subscriber (once per process).
pub fn init(dir: &Path, level: Level) -> Result<Redactor, OsError> {
    let (sub, redactor) = subscriber(dir, level)?;
    tracing::subscriber::set_global_default(sub)
        .map_err(|_| OsError::new("a log subscriber is already installed"))?;
    Ok(redactor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_token_shapes_and_bearer_and_registered_secrets() {
        let line = "token ghp_AbC123_xyz and github_pat_11AAA_bbb, Authorization: Bearer abc.DEF-1= end, custom hunter2secret!";
        let out = redact(line, &["hunter2secret".to_string()]);
        for leaked in ["ghp_AbC123", "github_pat_11", "abc.DEF", "hunter2secret"] {
            assert!(!out.contains(leaked), "{leaked} survived");
        }
        assert!(out.contains("Bearer [redacted]"));
        assert!(out.ends_with("!"));
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let line = "started runner ghm-o-r-abc123 for o/r (2/2)";
        assert_eq!(redact(line, &[]), line);
    }

    #[test]
    fn short_registered_secrets_are_ignored_to_avoid_mangling_logs() {
        assert_eq!(redact("abc abc", &["abc".to_string()]), "abc abc");
    }
}
