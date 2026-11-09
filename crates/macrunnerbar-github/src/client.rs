//! `GitHubClient`: the `GitHubApi` implementation.

use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use macrunnerbar_core::{
    ApiError, GitHubApi, JitConfig, JobConclusion, JobInfo, JobStatus, QueuedJob, Repo, RunnerInfo,
    RunnerRelease, Secret,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::RUNNER_PREFIX;
use crate::http::Http;
use crate::parse::{
    bool_field, check_repo, check_runner_name, check_version, normalize_labels, release_sha,
    str_field, u64_field,
};

const DEFAULT_BASE: &str = "https://api.github.com";
const RUNNER_REPO: &str = "actions/runner";
/// The runner group every repo-level runner belongs to.
const DEFAULT_RUNNER_GROUP: u64 = 1;

/// Blocking GitHub API client. Cheap to share behind `Arc`; the ETag cache is internal.
pub struct GitHubClient {
    http: Http,
}

impl fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubClient")
            .field("base_url", &self.http.base())
            .finish_non_exhaustive()
    }
}

impl GitHubClient {
    /// Client for `https://api.github.com`, honouring the proxy environment.
    pub fn new(token: Secret<String>) -> Self {
        Self {
            http: Http::new(token, DEFAULT_BASE, true),
        }
    }

    /// Client for another origin (a local stub in tests, or GitHub Enterprise). Ignores proxy variables.
    pub fn with_base_url(token: Secret<String>, base_url: &str) -> Self {
        Self {
            http: Http::new(token, base_url, false),
        }
    }

    fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
        v.get(key)
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

impl GitHubApi for GitHubClient {
    /// Every repo the token can see, with its `private` / `archived` / `disabled` / `admin` flags.
    /// Core's `policy::eligible` decides which are served.
    fn list_repos(&self) -> Result<Vec<Repo>, ApiError> {
        let mut out = Vec::new();
        self.http.get_all(
            "/user/repos?affiliation=owner,organization_member&per_page=100",
            |page| {
                let items = page
                    .as_array()
                    .ok_or_else(|| ApiError::Invalid("repo list is not an array".into()))?;
                for r in items {
                    out.push(Repo {
                        full_name: str_field(r, "full_name")?.to_string(),
                        private: bool_field(r, "private"),
                        archived: bool_field(r, "archived"),
                        disabled: bool_field(r, "disabled"),
                        admin: r.get("permissions").is_some_and(|p| bool_field(p, "admin")),
                    });
                }
                Ok(())
            },
        )?;
        Ok(out)
    }

    /// Jobs with status `queued` among the repo's queued and in-progress runs (an in-progress run can
    /// still have queued jobs), latest attempt only. Labels are returned as GitHub reports them.
    fn queued_jobs(&self, repo: &str) -> Result<Vec<QueuedJob>, ApiError> {
        check_repo(repo)?;
        let mut out: Vec<QueuedJob> = Vec::new();
        for status in ["queued", "in_progress"] {
            let runs = self.http.get(
                &format!("/repos/{repo}/actions/runs?status={status}&per_page=50"),
                true,
            )?;
            for run in Self::array(&runs.body, "workflow_runs") {
                let run_id = u64_field(run, "id")?;
                let workflow = run.get("name").and_then(Value::as_str).map(str::to_string);
                let run_url = run
                    .get("html_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let jobs = match self.http.get(
                    &format!("/repos/{repo}/actions/runs/{run_id}/jobs?filter=latest&per_page=100"),
                    true,
                ) {
                    Ok(p) => p,
                    // The run vanished between the two calls.
                    Err(ApiError::NotFound) => continue,
                    Err(e) => return Err(e),
                };
                for job in Self::array(&jobs.body, "jobs") {
                    if job.get("status").and_then(Value::as_str) != Some("queued") {
                        continue;
                    }
                    let job_id = u64_field(job, "id")?;
                    if out.iter().any(|j| j.job_id == job_id) {
                        continue;
                    }
                    out.push(QueuedJob {
                        repo: repo.to_string(),
                        run_id,
                        job_id,
                        workflow: workflow.clone(),
                        name: str_field(job, "name")?.to_string(),
                        labels: Self::array(job, "labels")
                            .iter()
                            .filter_map(|l| l.as_str().map(str::to_string))
                            .collect(),
                        html_url: run_url.to_string(),
                    });
                }
            }
        }
        Ok(out)
    }

    /// Register a single-use runner. `runner_name` must start with `ghm-`. Labels are lowercased and
    /// sent as the full set (include `self-hosted`), group 1, work folder `_work`.
    fn generate_jit(
        &self,
        repo: &str,
        runner_name: &str,
        labels: &[String],
    ) -> Result<JitConfig, ApiError> {
        check_repo(repo)?;
        check_runner_name(runner_name)?;
        let labels = normalize_labels(labels);
        if labels.is_empty() {
            return Err(ApiError::Invalid("at least one label is required".into()));
        }
        let body = json!({
            "name": runner_name,
            "runner_group_id": DEFAULT_RUNNER_GROUP,
            "labels": labels,
            "work_folder": "_work",
        });
        let resp = self.http.post_json(
            &format!("/repos/{repo}/actions/runners/generate-jitconfig"),
            &body,
        )?;
        let runner = resp
            .get("runner")
            .ok_or_else(|| ApiError::Invalid("no runner in JIT response".into()))?;
        let runner_id = u64_field(runner, "id")?;
        let encoded = str_field(&resp, "encoded_jit_config")?;
        let granted: Vec<&str> = Self::array(runner, "labels")
            .iter()
            .filter_map(|l| l.get("name").and_then(Value::as_str))
            .collect();
        tracing::debug!(repo, runner_id, labels = ?granted, "jit runner registered");
        Ok(JitConfig {
            runner_id,
            encoded: Secret::new(encoded.to_string()),
        })
    }

    /// Registered runners whose name starts with `ghm-`; other registrations (the Linux and Windows
    /// autoscalers') are never returned.
    fn list_runners(&self, repo: &str) -> Result<Vec<RunnerInfo>, ApiError> {
        check_repo(repo)?;
        let mut out = Vec::new();
        self.http.get_all(
            &format!("/repos/{repo}/actions/runners?per_page=100"),
            |page| {
                for r in Self::array(page, "runners") {
                    let name = str_field(r, "name")?;
                    if !name.starts_with(RUNNER_PREFIX) {
                        continue;
                    }
                    out.push(RunnerInfo {
                        id: u64_field(r, "id")?,
                        name: name.to_string(),
                        online: r.get("status").and_then(Value::as_str) == Some("online"),
                        busy: bool_field(r, "busy"),
                    });
                }
                Ok(())
            },
        )?;
        Ok(out)
    }

    /// Delete a registration, only if it is named `ghm-*` (checked with a GET first). A registration
    /// that is already gone is `Ok`.
    fn delete_runner(&self, repo: &str, runner_id: u64) -> Result<(), ApiError> {
        check_repo(repo)?;
        let path = format!("/repos/{repo}/actions/runners/{runner_id}");
        let runner = match self.http.get(&path, false) {
            Ok(p) => p,
            Err(ApiError::NotFound) => return Ok(()),
            Err(e) => return Err(e),
        };
        if !str_field(&runner.body, "name")?.starts_with(RUNNER_PREFIX) {
            return Err(ApiError::Forbidden(format!(
                "refusing to delete a runner that does not start with {RUNNER_PREFIX}"
            )));
        }
        match self.http.delete(&path) {
            Err(ApiError::NotFound) => Ok(()),
            other => other,
        }
    }

    /// Latest `actions/runner` release: osx-arm64 asset URL plus the SHA-256 from the raw release body.
    fn latest_runner_release(&self) -> Result<RunnerRelease, ApiError> {
        let page = self
            .http
            .get(&format!("/repos/{RUNNER_REPO}/releases/latest"), true)?;
        let rel = &page.body;
        let tag = str_field(rel, "tag_name")?;
        let version = tag.strip_prefix('v').unwrap_or(tag);
        check_version(version)?;
        let sha256 = release_sha(str_field(rel, "body")?)?;
        let asset_name = format!("actions-runner-osx-arm64-{version}.tar.gz");
        let url = Self::array(rel, "assets")
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some(asset_name.as_str()))
            .and_then(|a| a.get("browser_download_url").and_then(Value::as_str))
            .ok_or_else(|| ApiError::Invalid("release has no osx-arm64 asset".into()))?;
        Ok(RunnerRelease {
            version: version.to_string(),
            url: url.to_string(),
            sha256,
        })
    }

    /// Stream the asset (without the token) to `<dest>.part`, hash while writing, and rename to `dest`
    /// only when the SHA-256 matches. On any failure nothing is left behind.
    fn download_release(&self, release: &RunnerRelease, dest: &Path) -> Result<(), ApiError> {
        let part = part_path(dest);
        let result = self.download_to(release, &part);
        match result {
            Ok(()) => std::fs::rename(&part, dest).map_err(|e| {
                let _ = std::fs::remove_file(&part);
                ApiError::Invalid(format!("cannot move download into place: {:?}", e.kind()))
            }),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(e)
            }
        }
    }

    /// Looks through the repo's in-progress runs, then its ten most recent completed ones (latest
    /// attempt of each), for a job whose `runner_name` is this runner's. A runner takes one job, so
    /// the first match is the one.
    fn find_job_by_runner(
        &self,
        repo: &str,
        runner_name: &str,
    ) -> Result<Option<JobInfo>, ApiError> {
        check_repo(repo)?;
        check_runner_name(runner_name)?;
        for (status, per_page) in [("in_progress", 50), ("completed", 10)] {
            let runs = self.http.get(
                &format!("/repos/{repo}/actions/runs?status={status}&per_page={per_page}"),
                true,
            )?;
            for run in Self::array(&runs.body, "workflow_runs") {
                let run_id = u64_field(run, "id")?;
                let run_url = run
                    .get("html_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let jobs = match self.http.get(
                    &format!("/repos/{repo}/actions/runs/{run_id}/jobs?filter=latest&per_page=100"),
                    true,
                ) {
                    Ok(p) => p,
                    Err(ApiError::NotFound) => continue,
                    Err(e) => return Err(e),
                };
                for job in Self::array(&jobs.body, "jobs") {
                    if job.get("runner_name").and_then(Value::as_str) == Some(runner_name) {
                        return Self::parse_job(repo, run_url, job).map(Some);
                    }
                }
            }
        }
        Ok(None)
    }

    /// `GET /repos/{repo}/actions/jobs/{id}` (not cached: it is asked once per finished job).
    fn get_job(&self, repo: &str, job_id: u64) -> Result<JobInfo, ApiError> {
        check_repo(repo)?;
        let page = self
            .http
            .get(&format!("/repos/{repo}/actions/jobs/{job_id}"), false)?;
        Self::parse_job(repo, "", &page.body)
    }
}

impl GitHubClient {
    /// A job object (from the jobs list or `GET .../jobs/{id}`) as a [`JobInfo`].
    fn parse_job(repo: &str, run_url_fallback: &str, v: &Value) -> Result<JobInfo, ApiError> {
        let status = match v.get("status").and_then(Value::as_str) {
            Some("completed") => JobStatus::Completed,
            Some("in_progress") => JobStatus::InProgress,
            _ => JobStatus::Queued,
        };
        let conclusion = v
            .get("conclusion")
            .and_then(Value::as_str)
            .map(JobConclusion::from_api);
        Ok(JobInfo {
            repo: repo.to_string(),
            run_id: u64_field(v, "run_id")?,
            job_id: u64_field(v, "id")?,
            workflow: v
                .get("workflow_name")
                .and_then(Value::as_str)
                .map(str::to_string),
            name: str_field(v, "name")?.to_string(),
            html_url: v
                .get("html_url")
                .and_then(Value::as_str)
                .unwrap_or(run_url_fallback)
                .to_string(),
            status,
            conclusion: if status == JobStatus::Completed {
                conclusion
            } else {
                None
            },
        })
    }
}

impl GitHubClient {
    fn download_to(&self, release: &RunnerRelease, part: &Path) -> Result<(), ApiError> {
        let mut reader = self.http.open_download(&release.url)?;
        let mut file = File::create(part).map_err(|e| {
            ApiError::Invalid(format!("cannot create download file: {:?}", e.kind()))
        })?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|_| ApiError::Network("download interrupted".into()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])
                .map_err(|e| ApiError::Invalid(format!("cannot write download: {:?}", e.kind())))?;
        }
        file.flush()
            .map_err(|e| ApiError::Invalid(format!("cannot write download: {:?}", e.kind())))?;
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != release.sha256.to_ascii_lowercase() {
            return Err(ApiError::Invalid("checksum mismatch".into()));
        }
        Ok(())
    }
}
