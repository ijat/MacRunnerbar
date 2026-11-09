//! Pure policy: label matching, repo eligibility, cap accounting, per-repo backoff.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::types::{Repo, Settings, Timestamp, WaitReason, WaitingJobs, owner_of};

/// True only when `job_labels` is non-empty, every label is in `configured`, and `macos` is among
/// them (so a bare `self-hosted` job stays on Linux). Case-insensitive; surrounding whitespace ignored.
pub fn matches<A: AsRef<str>, B: AsRef<str>>(job_labels: &[A], configured: &[B]) -> bool {
    if job_labels.is_empty() {
        return false;
    }
    let configured: HashSet<String> = configured
        .iter()
        .map(|l| l.as_ref().trim().to_ascii_lowercase())
        .collect();
    let mut has_macos = false;
    for label in job_labels {
        let label = label.as_ref().trim().to_ascii_lowercase();
        if !configured.contains(&label) {
            return false;
        }
        has_macos |= label == "macos";
    }
    has_macos
}

fn listed(list: &[String], name: &str) -> bool {
    list.iter().any(|l| l.trim().eq_ignore_ascii_case(name))
}

/// Whether the app may serve jobs from `repo`: usable (not archived or disabled), admin rights,
/// not excluded, inside the include list when one is set, and public only when allowed.
pub fn eligible(repo: &Repo, settings: &Settings) -> bool {
    if repo.archived || repo.disabled || !repo.admin {
        return false;
    }
    if listed(&settings.exclude, &repo.full_name) {
        return false;
    }
    if !settings.include.is_empty() && !listed(&settings.include, &repo.full_name) {
        return false;
    }
    repo.private || listed(&settings.public_allow, &repo.full_name)
}

/// Eligible repo names, sorted.
pub fn eligible_repos(repos: &[Repo], settings: &Settings) -> Vec<String> {
    let mut out: Vec<String> = repos
        .iter()
        .filter(|r| eligible(r, settings))
        .map(|r| r.full_name.clone())
        .collect();
    out.sort();
    out
}

/// Queued work for one repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoDemand {
    /// `owner/name`.
    pub repo: String,
    /// Queued jobs in the repo that match our labels.
    pub queued: usize,
    /// Runners we already started for this repo that have not taken a job yet.
    pub unclaimed: usize,
}

/// Runners currently in flight.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Load {
    /// All runners.
    pub total: usize,
    /// Runners per owner, owner lowercased.
    pub per_owner: HashMap<String, usize>,
}

impl Load {
    /// Count runners from the `owner/name` of each.
    pub fn from_repos<'a>(repos: impl IntoIterator<Item = &'a str>) -> Self {
        let mut load = Self::default();
        for repo in repos {
            load.total += 1;
            *load
                .per_owner
                .entry(owner_of(repo).to_ascii_lowercase())
                .or_default() += 1;
        }
        load
    }
}

/// What to start and what waits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnPlan {
    /// One entry per runner to start (a repo repeats once per runner).
    pub spawn: Vec<String>,
    /// Jobs held back by a cap.
    pub waiting: Vec<WaitingJobs>,
}

/// Decide how many runners to start for `demands`, in order. For each repo the unmet demand is
/// `queued - unclaimed`; it is clipped by the per-owner cap first, then by what is left of the global cap.
pub fn plan_spawns(
    demands: &[RepoDemand],
    load: &Load,
    per_owner_max: usize,
    global_max: usize,
) -> SpawnPlan {
    let mut total = load.total;
    let mut per_owner = load.per_owner.clone();
    let mut plan = SpawnPlan::default();
    for d in demands {
        let want = d.queued.saturating_sub(d.unclaimed);
        if want == 0 {
            continue;
        }
        let owner = owner_of(&d.repo).to_ascii_lowercase();
        let owner_room = per_owner_max.saturating_sub(per_owner.get(&owner).copied().unwrap_or(0));
        let global_room = global_max.saturating_sub(total);
        let start = want.min(owner_room).min(global_room);
        for _ in 0..start {
            plan.spawn.push(d.repo.clone());
        }
        total += start;
        *per_owner.entry(owner).or_default() += start;
        if start < want {
            let reason = if owner_room <= global_room {
                WaitReason::OwnerCap
            } else {
                WaitReason::GlobalCap
            };
            plan.waiting.push(WaitingJobs {
                repo: d.repo.clone(),
                count: want - start,
                reason,
            });
        }
    }
    plan
}

/// Exponential backoff parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    /// Delay after the first failure.
    pub base: Duration,
    /// Upper bound of the delay.
    pub max: Duration,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(60),
            max: Duration::from_secs(600),
        }
    }
}

impl BackoffPolicy {
    /// Delay after `failures` consecutive failures: `base * 2^(failures-1)`, capped at `max`; zero for none.
    pub fn delay(&self, failures: u32) -> Duration {
        if failures == 0 {
            return Duration::ZERO;
        }
        let factor = 1u32.checked_shl(failures - 1).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.max)
    }
}

/// Per-repo backoff state.
#[derive(Debug, Clone, Default)]
pub struct RepoBackoff {
    policy: BackoffPolicy,
    state: HashMap<String, (u32, Timestamp)>,
}

impl RepoBackoff {
    /// With the given policy.
    pub fn new(policy: BackoffPolicy) -> Self {
        Self {
            policy,
            state: HashMap::new(),
        }
    }

    /// Record a failure for `repo` at `now`; returns the delay until it is polled again.
    pub fn record_failure(&mut self, repo: &str, now: Timestamp) -> Duration {
        let entry = self
            .state
            .entry(repo.to_ascii_lowercase())
            .or_insert((0, now));
        entry.0 = entry.0.saturating_add(1);
        let delay = self.policy.delay(entry.0);
        entry.1 = now.plus(delay);
        delay
    }

    /// Record a success: forget the failures.
    pub fn record_success(&mut self, repo: &str) {
        self.state.remove(&repo.to_ascii_lowercase());
    }

    /// Whether `repo` should be skipped at `now`.
    pub fn is_backed_off(&self, repo: &str, now: Timestamp) -> bool {
        self.state
            .get(&repo.to_ascii_lowercase())
            .is_some_and(|&(_, until)| now < until)
    }

    /// Consecutive failures recorded for `repo`.
    pub fn failures(&self, repo: &str) -> u32 {
        self.state
            .get(&repo.to_ascii_lowercase())
            .map_or(0, |&(n, _)| n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: [&str; 3] = ["self-hosted", "macos", "arm64"];

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn matches_requires_subset_and_macos() {
        assert!(matches(&["self-hosted", "macos", "arm64"], &SET));
        assert!(matches(&["macos"], &SET));
        assert!(matches(&["self-hosted", "macOS"], &SET));
        assert!(matches(&[" ARM64 ", "MacOS"], &SET));
    }

    #[test]
    fn bare_self_hosted_does_not_match() {
        assert!(!matches(&["self-hosted"], &SET));
        assert!(!matches(&["self-hosted", "arm64"], &SET));
    }

    #[test]
    fn matches_rejects_empty_and_foreign_labels() {
        assert!(!matches::<&str, _>(&[], &SET));
        assert!(!matches(&["macos", "gpu"], &SET));
        assert!(!matches(&["ubuntu-latest"], &SET));
        assert!(!matches(&["macos"], &["self-hosted", "arm64"]));
        assert!(!matches(&["macos"], &[] as &[&str]));
    }

    fn repo(name: &str, private: bool) -> Repo {
        Repo {
            full_name: name.into(),
            private,
            archived: false,
            disabled: false,
            admin: true,
        }
    }

    #[test]
    fn public_repos_are_skipped_unless_allowed() {
        let mut st = Settings::default();
        assert!(eligible(&repo("o/priv", true), &st));
        assert!(!eligible(&repo("o/pub", false), &st));
        st.public_allow = s(&["O/Pub"]);
        assert!(eligible(&repo("o/pub", false), &st));
    }

    #[test]
    fn eligibility_respects_state_admin_include_exclude() {
        let mut st = Settings::default();
        let mut r = repo("o/r", true);
        r.archived = true;
        assert!(!eligible(&r, &st));
        r.archived = false;
        r.disabled = true;
        assert!(!eligible(&r, &st));
        r.disabled = false;
        r.admin = false;
        assert!(!eligible(&r, &st));
        r.admin = true;
        st.exclude = s(&["O/R"]);
        assert!(!eligible(&r, &st));
        st.exclude.clear();
        st.include = s(&["o/other"]);
        assert!(!eligible(&r, &st));
        st.include = s(&["O/r"]);
        assert!(eligible(&r, &st));
    }

    #[test]
    fn eligible_repos_sorted() {
        let repos = [repo("b/x", true), repo("a/y", true), repo("c/pub", false)];
        assert_eq!(
            eligible_repos(&repos, &Settings::default()),
            vec!["a/y", "b/x"]
        );
    }

    fn demand(repo: &str, queued: usize) -> RepoDemand {
        RepoDemand {
            repo: repo.into(),
            queued,
            unclaimed: 0,
        }
    }

    #[test]
    fn global_cap_starts_two_of_three_and_reports_one_waiting() {
        let plan = plan_spawns(&[demand("o/r", 3)], &Load::default(), 8, 2);
        assert_eq!(plan.spawn, vec!["o/r", "o/r"]);
        assert_eq!(
            plan.waiting,
            vec![WaitingJobs {
                repo: "o/r".into(),
                count: 1,
                reason: WaitReason::GlobalCap
            }]
        );
    }

    #[test]
    fn owner_cap_applies_before_global() {
        // owner cap 1, global 5: owner "a" gets one runner even across two repos; "b" is unaffected.
        let demands = [demand("a/x", 2), demand("a/y", 1), demand("b/z", 1)];
        let plan = plan_spawns(&demands, &Load::default(), 1, 5);
        assert_eq!(plan.spawn, vec!["a/x", "b/z"]);
        assert_eq!(plan.waiting.len(), 2);
        assert!(
            plan.waiting
                .iter()
                .all(|w| w.reason == WaitReason::OwnerCap)
        );
        assert_eq!(plan.waiting.iter().map(|w| w.count).sum::<usize>(), 2);
    }

    #[test]
    fn owner_held_back_jobs_do_not_consume_global_room() {
        let demands = [demand("a/x", 3), demand("b/y", 1)];
        let plan = plan_spawns(&demands, &Load::default(), 1, 2);
        assert_eq!(plan.spawn, vec!["a/x", "b/y"]);
    }

    #[test]
    fn existing_load_counts_against_caps() {
        let load = Load::from_repos(["A/x", "b/y"]);
        assert_eq!(load.total, 2);
        assert_eq!(load.per_owner["a"], 1);
        let plan = plan_spawns(&[demand("a/x", 1)], &load, 1, 5);
        assert!(plan.spawn.is_empty());
        assert_eq!(plan.waiting[0].reason, WaitReason::OwnerCap);
        let plan = plan_spawns(&[demand("c/x", 1)], &load, 5, 2);
        assert!(plan.spawn.is_empty());
        assert_eq!(plan.waiting[0].reason, WaitReason::GlobalCap);
    }

    #[test]
    fn unclaimed_runners_reduce_demand() {
        let d = RepoDemand {
            repo: "o/r".into(),
            queued: 2,
            unclaimed: 2,
        };
        assert_eq!(
            plan_spawns(&[d], &Load::default(), 8, 8),
            SpawnPlan::default()
        );
        let d = RepoDemand {
            repo: "o/r".into(),
            queued: 1,
            unclaimed: 3,
        };
        assert_eq!(
            plan_spawns(&[d], &Load::default(), 8, 8),
            SpawnPlan::default()
        );
    }

    #[test]
    fn zero_caps_start_nothing() {
        let plan = plan_spawns(&[demand("o/r", 2)], &Load::default(), 0, 0);
        assert!(plan.spawn.is_empty());
        assert_eq!(plan.waiting[0].count, 2);
    }

    #[test]
    fn backoff_doubles_up_to_max() {
        let p = BackoffPolicy {
            base: Duration::from_secs(60),
            max: Duration::from_secs(600),
        };
        let secs: Vec<u64> = (0..8).map(|n| p.delay(n).as_secs()).collect();
        assert_eq!(secs, vec![0, 60, 120, 240, 480, 600, 600, 600]);
        assert_eq!(p.delay(u32::MAX), Duration::from_secs(600));
    }

    #[test]
    fn repo_backoff_tracks_failures_and_resets_on_success() {
        let mut b = RepoBackoff::default();
        let t0 = Timestamp::from_millis(1_000_000);
        assert!(!b.is_backed_off("o/r", t0));
        assert_eq!(b.record_failure("o/r", t0), Duration::from_secs(60));
        assert!(b.is_backed_off("O/R", t0.plus(Duration::from_secs(59))));
        assert!(!b.is_backed_off("o/r", t0.plus(Duration::from_secs(60))));
        let t1 = t0.plus(Duration::from_secs(60));
        assert_eq!(b.record_failure("o/r", t1), Duration::from_secs(120));
        assert_eq!(b.failures("o/r"), 2);
        assert!(!b.is_backed_off("o/other", t1));
        b.record_success("o/r");
        assert_eq!(b.failures("o/r"), 0);
        assert!(!b.is_backed_off("o/r", t1));
        assert_eq!(b.record_failure("o/r", t1), Duration::from_secs(60));
    }
}
