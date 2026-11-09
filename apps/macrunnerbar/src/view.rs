//! `StatusSnapshot` to what the popover shows: pure, no UI types.

use std::time::Duration;

use macrunnerbar_core::{IconState, StatusSnapshot, Timestamp, WaitReason};

/// Jobs listed before the rest collapse into "+n more".
pub(crate) const MAX_JOB_ROWS: usize = 8;
/// Waiting lines listed before the rest collapse.
pub(crate) const MAX_WAIT_ROWS: usize = 3;

/// How the state reads in the header (a tone, not a colour: the UI maps it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mood {
    Ok,
    Busy,
    Warn,
    Paused,
}

/// One running job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobRow {
    /// `owner/name`.
    pub(crate) repo: String,
    /// `workflow / job`, or what the runner is doing before it has a job.
    pub(crate) detail: String,
    /// `3m 12s`.
    pub(crate) elapsed: String,
    /// Where a click goes; only ever a github.com URL.
    pub(crate) url: Option<String>,
}

/// What the popover renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PopoverView {
    pub(crate) title: String,
    pub(crate) state_text: String,
    pub(crate) mood: Mood,
    /// `n/max` runners.
    pub(crate) runners: String,
    pub(crate) paused: bool,
    pub(crate) jobs: Vec<JobRow>,
    /// Running jobs beyond [`MAX_JOB_ROWS`].
    pub(crate) more_jobs: usize,
    /// One line per repo with jobs held back by a cap.
    pub(crate) waiting: Vec<String>,
    pub(crate) more_waiting: usize,
    pub(crate) last_error: Option<String>,
}

/// `42s`, `3m 12s`, `1h 05m`.
pub(crate) fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// A job's link is opened only when it is an `https://github.com/` URL: the text comes from an API
/// response, and `open` would launch whatever a `file:` or app URL names.
pub(crate) fn safe_url(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://github.com/")?;
    let ok = !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~/?=&%#:+@".contains(&b));
    ok.then_some(url)
}

/// The tray tooltip: the state in words (the second channel next to the icon's drawing).
pub(crate) fn tooltip(s: &StatusSnapshot) -> String {
    let base = match s.icon_state() {
        IconState::Paused => "MacRunnerbar - paused".to_string(),
        IconState::Error => "MacRunnerbar - error".to_string(),
        IconState::Running => {
            format!(
                "MacRunnerbar - {} running ({}/{})",
                s.running_count(),
                s.running_count(),
                s.max_runners
            )
        }
        IconState::Idle => "MacRunnerbar - idle".to_string(),
    };
    match &s.last_error {
        Some(e) if s.icon_state() == IconState::Error => format!("{base}: {}", clip(e, 120)),
        _ => base,
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}\u{2026}")
    }
}

/// The popover's content for `s` at `now`.
pub(crate) fn popover_view(s: &StatusSnapshot, now: Timestamp) -> PopoverView {
    let icon = s.icon_state();
    let (state_text, mood) = match icon {
        IconState::Paused => ("Paused".to_string(), Mood::Paused),
        IconState::Error => ("Needs attention".to_string(), Mood::Warn),
        IconState::Running => (format!("{} running", s.running_count()), Mood::Busy),
        IconState::Idle => ("Idle".to_string(), Mood::Ok),
    };
    let jobs: Vec<JobRow> = s
        .running
        .iter()
        .take(MAX_JOB_ROWS)
        .map(|j| {
            let detail = match (&j.workflow, &j.job_name) {
                (Some(w), Some(n)) => format!("{w} / {n}"),
                (None, Some(n)) => n.clone(),
                _ => "waiting for a job".to_string(),
            };
            JobRow {
                repo: j.repo.clone(),
                detail,
                elapsed: format_elapsed(now.since(j.started)),
                url: j.html_url.as_deref().and_then(safe_url).map(str::to_string),
            }
        })
        .collect();
    let waiting: Vec<String> = s
        .waiting
        .iter()
        .take(MAX_WAIT_ROWS)
        .map(|w| {
            let why = match w.reason {
                WaitReason::OwnerCap => "owner cap",
                WaitReason::GlobalCap => "runner cap",
            };
            let noun = if w.count == 1 { "job" } else { "jobs" };
            format!("{} {noun} waiting on the {why}: {}", w.count, w.repo)
        })
        .collect();
    PopoverView {
        title: "MacRunnerbar".to_string(),
        state_text,
        mood,
        runners: format!("{}/{}", s.running_count(), s.max_runners),
        paused: s.paused,
        more_jobs: s.running.len().saturating_sub(MAX_JOB_ROWS),
        jobs,
        more_waiting: s.waiting.len().saturating_sub(MAX_WAIT_ROWS),
        waiting,
        last_error: s.last_error.as_deref().map(|e| clip(e, 160)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macrunnerbar_core::{RunningJob, WaitingJobs};

    fn t(ms: u64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    fn running(repo: &str, started_ms: u64, job: Option<&str>, url: Option<&str>) -> RunningJob {
        RunningJob {
            runner_name: format!("ghm-{}-000000", repo.replace('/', "-")),
            repo: repo.into(),
            workflow: job.map(|_| "CI".to_string()),
            job_name: job.map(str::to_string),
            html_url: url.map(str::to_string),
            started: t(started_ms),
        }
    }

    #[test]
    fn elapsed_formats() {
        let f = |s| format_elapsed(Duration::from_secs(s));
        assert_eq!(f(0), "0s");
        assert_eq!(f(59), "59s");
        assert_eq!(f(60), "1m 00s");
        assert_eq!(f(192), "3m 12s");
        assert_eq!(f(3599), "59m 59s");
        assert_eq!(f(3600), "1h 00m");
        assert_eq!(f(3600 * 5 + 65), "5h 01m");
    }

    #[test]
    fn only_github_https_links_are_openable() {
        assert!(safe_url("https://github.com/o/r/actions/runs/1/job/2").is_some());
        for bad in [
            "http://github.com/o/r",
            "file:///etc/passwd",
            "https://evil.example/https://github.com/",
            "https://github.com.evil.example/x",
            "https://github.com/",
            "https://github.com/o r",
            "https://github.com/o/r\n-a Calculator",
            "x-apple.systempreferences:",
            "",
        ] {
            assert!(safe_url(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn running_snapshot_maps_to_rows_with_elapsed_and_links() {
        let mut s = StatusSnapshot::idle(2, t(200_000));
        s.running.push(running(
            "o/a",
            100_000,
            Some("build"),
            Some("https://github.com/o/a/actions/runs/1"),
        ));
        s.running.push(running("o/b", 190_000, None, None));
        let v = popover_view(&s, t(200_000));
        assert_eq!(v.state_text, "2 running");
        assert_eq!(v.mood, Mood::Busy);
        assert_eq!(v.runners, "2/2");
        assert_eq!(v.jobs[0].repo, "o/a");
        assert_eq!(v.jobs[0].detail, "CI / build");
        assert_eq!(v.jobs[0].elapsed, "1m 40s");
        assert_eq!(
            v.jobs[0].url.as_deref(),
            Some("https://github.com/o/a/actions/runs/1")
        );
        assert_eq!(v.jobs[1].detail, "waiting for a job");
        assert_eq!(v.jobs[1].url, None);
        assert_eq!(v.jobs[1].elapsed, "10s");
        assert!(v.last_error.is_none() && !v.paused);
    }

    #[test]
    fn an_unsafe_job_url_is_dropped_not_shown_as_a_link() {
        let mut s = StatusSnapshot::idle(2, t(0));
        s.running.push(running(
            "o/a",
            0,
            Some("x"),
            Some("file:///Applications/Calculator.app"),
        ));
        assert_eq!(popover_view(&s, t(0)).jobs[0].url, None);
    }

    #[test]
    fn waiting_lines_name_the_cap_and_long_lists_collapse() {
        let mut s = StatusSnapshot::idle(2, t(0));
        s.waiting.push(WaitingJobs {
            repo: "o/a".into(),
            count: 1,
            reason: WaitReason::GlobalCap,
        });
        s.waiting.push(WaitingJobs {
            repo: "o/b".into(),
            count: 3,
            reason: WaitReason::OwnerCap,
        });
        let v = popover_view(&s, t(0));
        assert_eq!(v.waiting[0], "1 job waiting on the runner cap: o/a");
        assert_eq!(v.waiting[1], "3 jobs waiting on the owner cap: o/b");
        for i in 0..6 {
            s.running.push(running(&format!("o/r{i}"), 0, None, None));
        }
        for i in 0..5 {
            s.waiting.push(WaitingJobs {
                repo: format!("o/w{i}"),
                count: 1,
                reason: WaitReason::GlobalCap,
            });
        }
        s.running
            .extend((0..5).map(|i| running(&format!("o/x{i}"), 0, None, None)));
        let v = popover_view(&s, t(0));
        assert_eq!(v.jobs.len(), MAX_JOB_ROWS);
        assert_eq!(v.more_jobs, 11 - MAX_JOB_ROWS);
        assert_eq!(v.waiting.len(), MAX_WAIT_ROWS);
        assert_eq!(v.more_waiting, 7 - MAX_WAIT_ROWS);
    }

    #[test]
    fn state_text_and_mood_follow_the_icon_state() {
        let mut s = StatusSnapshot::idle(2, t(0));
        assert_eq!(
            (
                popover_view(&s, t(0)).state_text.as_str(),
                popover_view(&s, t(0)).mood
            ),
            ("Idle", Mood::Ok)
        );
        s.last_error = Some("o/r: HTTP 500".into());
        let v = popover_view(&s, t(0));
        assert_eq!(
            (v.state_text.as_str(), v.mood),
            ("Needs attention", Mood::Warn)
        );
        assert_eq!(v.last_error.as_deref(), Some("o/r: HTTP 500"));
        s.paused = true;
        let v = popover_view(&s, t(0));
        assert_eq!(
            (v.state_text.as_str(), v.mood, v.paused),
            ("Paused", Mood::Paused, true)
        );
    }

    #[test]
    fn tooltip_names_the_state_in_words() {
        let mut s = StatusSnapshot::idle(2, t(0));
        assert_eq!(tooltip(&s), "MacRunnerbar - idle");
        s.running.push(running("o/a", 0, None, None));
        assert_eq!(tooltip(&s), "MacRunnerbar - 1 running (1/2)");
        s.last_error = Some("boom".into());
        assert_eq!(tooltip(&s), "MacRunnerbar - error: boom");
        s.paused = true;
        assert_eq!(tooltip(&s), "MacRunnerbar - paused");
        assert!(
            tooltip(&{
                let mut e = StatusSnapshot::idle(2, t(0));
                e.last_error = Some("x".repeat(500));
                e
            })
            .chars()
            .count()
                < 160
        );
    }
}
