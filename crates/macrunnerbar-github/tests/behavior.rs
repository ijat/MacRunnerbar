//! Adapter behaviour against the in-process stub: ETag, pagination, typed errors, token hygiene,
//! the `ghm-` guard, download hygiene. Nothing here touches real GitHub.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use macrunnerbar_core::{ApiError, GitHubApi, RunnerRelease, Secret};
use macrunnerbar_github::GitHubClient;
use serde_json::json;
use sha2::{Digest, Sha256};
use support::{Req, Resp, Stub};

const TOKEN: &str = "ghp_SENTINEL_TOKEN_0123456789abcdefABCDEF";

fn client(stub: &Stub) -> GitHubClient {
    GitHubClient::with_base_url(Secret::new(TOKEN.into()), stub.url())
}

fn repo_json(name: &str) -> serde_json::Value {
    json!({"full_name":name,"private":true,"archived":false,"disabled":false,"permissions":{"admin":true}})
}

#[test]
fn list_repos_maps_fields_and_follows_pages() {
    let stub = Stub::start(|req, base| {
        if req.query().contains("page=2") {
            Resp::json(
                200,
                &json!([{"full_name":"o/two","private":false,"archived":true,"disabled":true}]),
            )
        } else {
            Resp::json(200, &json!([repo_json("o/one")]))
                .header("Link", &format!("<{base}/user/repos?page=2>; rel=\"next\""))
        }
    });
    let repos = client(&stub).list_repos().expect("repos");
    assert_eq!(repos.len(), 2);
    assert!(repos[0].private && repos[0].admin && !repos[0].archived);
    assert!(!repos[1].private && repos[1].archived && repos[1].disabled && !repos[1].admin);
}

#[test]
fn pagination_never_leaves_the_api_origin() {
    let stub = Stub::start(|_, _| {
        Resp::json(200, &json!([repo_json("o/one")]))
            .header("Link", "<https://evil.example/steal>; rel=\"next\"")
    });
    let err = client(&stub)
        .list_repos()
        .expect_err("foreign link rejected");
    assert!(matches!(err, ApiError::Invalid(_)), "{err}");
    assert_eq!(stub.requests().len(), 1, "no request went anywhere else");
}

#[test]
fn etag_is_sent_back_and_304_replays_the_cache_including_next_link() {
    let hits = Arc::new(AtomicUsize::new(0));
    let h2 = hits.clone();
    let stub = Stub::start(move |req, base| {
        h2.fetch_add(1, Ordering::SeqCst);
        let page2 = req.query().contains("page=2");
        let etag = if page2 {
            "\"etag-two\""
        } else {
            "W/\"etag-one\""
        };
        if req.header("if-none-match") == Some(etag) {
            return Resp::empty(304);
        }
        if page2 {
            Resp::json(200, &json!([repo_json("o/two")])).header("ETag", etag)
        } else {
            Resp::json(200, &json!([repo_json("o/one")]))
                .header("ETag", etag)
                .header("Link", &format!("<{base}/user/repos?page=2>; rel=\"next\""))
        }
    });
    let c = client(&stub);
    let first = c.list_repos().expect("first");
    let second = c.list_repos().expect("second, all 304");
    assert_eq!(first, second);
    assert_eq!(second.len(), 2);
    let reqs = stub.requests();
    assert_eq!(reqs.len(), 4);
    assert_eq!(reqs[2].header("if-none-match"), Some("W/\"etag-one\""));
    assert_eq!(reqs[3].header("if-none-match"), Some("\"etag-two\""));
    assert_eq!(hits.load(Ordering::SeqCst), 4);
}

#[test]
fn changed_etag_returns_fresh_body() {
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    let stub = Stub::start(move |_, _| {
        let i = n2.fetch_add(1, Ordering::SeqCst);
        Resp::json(200, &json!([repo_json(&format!("o/r{i}"))]))
            .header("ETag", &format!("\"e{i}\""))
    });
    let c = client(&stub);
    assert_eq!(c.list_repos().expect("1")[0].full_name, "o/r0");
    assert_eq!(c.list_repos().expect("2")[0].full_name, "o/r1");
}

#[test]
fn status_codes_map_to_typed_errors() {
    let status = Arc::new(Mutex::new((
        200u16,
        Vec::<(String, String)>::new(),
        json!({}),
    )));
    let s2 = status.clone();
    let stub = Stub::start(move |_, _| {
        let (code, headers, body) = s2.lock().expect("lock").clone();
        let mut r = Resp::json(code, &body);
        for (k, v) in headers {
            r = r.header(&k, &v);
        }
        r
    });
    let c = client(&stub);
    let set = |code: u16, headers: &[(&str, &str)], body: serde_json::Value| {
        *status.lock().expect("lock") = (
            code,
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body,
        );
    };
    let call = || c.queued_jobs("o/r").expect_err("error expected");

    set(401, &[], json!({"message":"Bad credentials"}));
    assert_eq!(call(), ApiError::Unauthorized);
    set(404, &[], json!({}));
    assert_eq!(call(), ApiError::NotFound);
    set(500, &[], json!({}));
    assert_eq!(call(), ApiError::Http { status: 500 });
    set(502, &[], json!({}));
    assert_eq!(call(), ApiError::Http { status: 502 });
    set(301, &[("Location", "https://evil.example/")], json!({}));
    assert_eq!(
        call(),
        ApiError::Http { status: 301 },
        "redirects are not followed"
    );
    set(
        403,
        &[],
        json!({"message":"Resource not accessible by personal access token"}),
    );
    assert!(matches!(call(), ApiError::Forbidden(m) if m.contains("not accessible")));
    set(
        403,
        &[("x-ratelimit-remaining", "0"), ("retry-after", "30")],
        json!({"message":"API rate limit exceeded"}),
    );
    assert_eq!(
        call(),
        ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(30))
        }
    );
    set(
        403,
        &[],
        json!({"message":"You have exceeded a secondary rate limit."}),
    );
    assert_eq!(call(), ApiError::RateLimited { retry_after: None });
    set(429, &[("retry-after", "7")], json!({}));
    assert_eq!(
        call(),
        ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(7))
        }
    );
    set(
        403,
        &[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1")],
        json!({}),
    );
    assert!(matches!(
        call(),
        ApiError::RateLimited {
            retry_after: Some(_)
        }
    ));
}

#[test]
fn rate_limited_is_not_global_but_unauthorized_and_network_are() {
    assert!(ApiError::Unauthorized.is_global());
    assert!(!ApiError::RateLimited { retry_after: None }.is_global());
}

#[test]
fn transport_failure_is_a_network_error() {
    let stub = Stub::start(|_, _| Resp::hangup());
    let err = client(&stub).list_repos().expect_err("hangup");
    assert!(matches!(err, ApiError::Network(_)), "{err}");
    // Nothing listening at all.
    let dead = {
        let s = Stub::start(|_, _| Resp::empty(200));
        s.url().to_string()
    };
    let c = GitHubClient::with_base_url(Secret::new(TOKEN.into()), &dead);
    assert!(matches!(c.list_repos(), Err(ApiError::Network(_))));
}

/// A hostile server that echoes the whole request (token included) into every error body and
/// status text. No error string, `Debug` output or request URL may contain the token.
#[test]
fn token_never_appears_in_any_error_debug_or_url() {
    let stub = Stub::start(|req: &Req, _| {
        let echo = req.header_dump();
        match req.target.as_str() {
            t if t.contains("/403") => Resp::json(403, &json!({"message": echo})),
            t if t.contains("/ratelimit") => {
                Resp::json(403, &json!({"message": format!("rate limit {echo}")}))
            }
            t if t.contains("/500") => Resp::json(500, &json!({"message": echo})),
            t if t.contains("/401") => Resp::json(401, &json!({"message": echo})),
            t if t.contains("/badjson") => Resp::bytes(200, echo.into_bytes()),
            t if t.contains("/hangup") => Resp::hangup(),
            _ => Resp::json(404, &json!({"message": echo})),
        }
    });
    let c = client(&stub);
    let mut texts: Vec<String> = vec![format!("{c:?}")];
    for repo in [
        "o/403",
        "o/ratelimit",
        "o/500",
        "o/401",
        "o/badjson",
        "o/hangup",
        "o/other",
    ] {
        let e = c.queued_jobs(repo).expect_err("must fail");
        texts.push(format!("{e}"));
        texts.push(format!("{e:?}"));
    }
    let e = c
        .generate_jit("o/403", "ghm-o-x-abc123", &["macos".into()])
        .expect_err("jit fails");
    texts.push(format!("{e} {e:?}"));
    let e = c.delete_runner("o/401", 5).expect_err("delete fails");
    texts.push(format!("{e} {e:?}"));
    let e = c.list_runners("o/500").expect_err("list fails");
    texts.push(format!("{e} {e:?}"));
    for t in &texts {
        assert!(
            !t.contains(TOKEN),
            "token leaked into: {}",
            t.replace(TOKEN, "<TOKEN>")
        );
    }
    assert!(texts.len() > 10);
    // The token was actually sent as a header (so the test echoes something real) and never in a URL.
    let reqs = stub.requests();
    assert!(
        reqs.iter()
            .all(|r| r.header("authorization") == Some(&format!("Bearer {TOKEN}")))
    );
    assert!(reqs.iter().all(|r| !r.target.contains(TOKEN)));
}

#[test]
fn echoed_token_in_forbidden_message_is_scrubbed() {
    let stub =
        Stub::start(|_, _| Resp::json(403, &json!({"message": format!("denied for {TOKEN}")})));
    match client(&stub).queued_jobs("o/r") {
        Err(ApiError::Forbidden(m)) => {
            assert!(m.contains("[redacted]") && !m.contains(TOKEN), "{m}");
        }
        other => panic!("expected Forbidden, got {other:?}"),
    }
}

#[test]
fn queued_jobs_collects_queued_jobs_from_queued_and_in_progress_runs_without_duplicates() {
    let stub = Stub::start(|req, _| {
        let p = req.path().to_string();
        if p.ends_with("/actions/runs") {
            // The same run shows up under both statuses; run 2 is in progress with one queued job.
            if req.query().contains("status=queued") {
                Resp::json(
                    200,
                    &json!({"workflow_runs":[{"id":1,"name":"CI","html_url":"u1"}]}),
                )
            } else {
                Resp::json(
                    200,
                    &json!({"workflow_runs":[{"id":1,"name":"CI","html_url":"u1"},{"id":2,"name":"Release","html_url":"u2"}]}),
                )
            }
        } else if p.ends_with("/runs/1/jobs") {
            Resp::json(
                200,
                &json!({"jobs":[{"id":10,"name":"a","status":"queued","labels":["macos"]}]}),
            )
        } else if p.ends_with("/runs/2/jobs") {
            Resp::json(
                200,
                &json!({"jobs":[
                    {"id":20,"name":"b","status":"in_progress","labels":["macos"]},
                    {"id":21,"name":"c","status":"queued","labels":["self-hosted","macOS","arm64"]}
                ]}),
            )
        } else {
            Resp::json(404, &json!({}))
        }
    });
    let jobs = client(&stub).queued_jobs("o/r").expect("jobs");
    let ids: Vec<u64> = jobs.iter().map(|j| j.job_id).collect();
    assert_eq!(ids, vec![10, 21]);
    assert_eq!(jobs[1].workflow.as_deref(), Some("Release"));
    assert_eq!(jobs[1].labels, vec!["self-hosted", "macOS", "arm64"]);
    assert_eq!(jobs[1].html_url, "u2");
    assert!(
        stub.requests()
            .iter()
            .any(|r| r.target.contains("filter=latest"))
    );
}

#[test]
fn queued_jobs_skips_a_run_that_vanished() {
    let stub = Stub::start(|req, _| {
        if req.path().ends_with("/actions/runs") && req.query().contains("status=queued") {
            Resp::json(
                200,
                &json!({"workflow_runs":[{"id":1,"name":"CI","html_url":"u"}]}),
            )
        } else if req.path().ends_with("/actions/runs") {
            Resp::json(200, &json!({"workflow_runs":[]}))
        } else {
            Resp::json(404, &json!({"message":"Not Found"}))
        }
    });
    assert!(client(&stub).queued_jobs("o/r").expect("ok").is_empty());
}

#[test]
fn invalid_repo_names_never_reach_the_network() {
    let stub = Stub::start(|_, _| Resp::json(200, &json!({})));
    let c = client(&stub);
    for bad in ["o/r?x=1", "../../user", "o", "o/r/extra", "o/r#frag"] {
        assert!(
            matches!(c.queued_jobs(bad), Err(ApiError::Invalid(_))),
            "{bad}"
        );
        assert!(
            matches!(c.list_runners(bad), Err(ApiError::Invalid(_))),
            "{bad}"
        );
        assert!(
            matches!(c.delete_runner(bad, 1), Err(ApiError::Invalid(_))),
            "{bad}"
        );
    }
    assert!(stub.requests().is_empty());
}

#[test]
fn generate_jit_request_shape_and_label_normalisation() {
    let stub = Stub::start(|req, _| {
        assert_eq!(req.method, "POST");
        Resp::json(
            201,
            &json!({"runner":{"id":55,"name":"x","labels":[{"name":"self-hosted"},{"name":"macos"},{"name":"arm64"}]},"encoded_jit_config":"SECRETJIT=="}),
        )
    });
    let jit = client(&stub)
        .generate_jit(
            "o/r",
            "ghm-o-r-abc123",
            &[
                "Self-Hosted".into(),
                "macOS".into(),
                "ARM64".into(),
                "macos".into(),
            ],
        )
        .expect("jit");
    assert_eq!(jit.runner_id, 55);
    assert_eq!(jit.encoded.expose(), "SECRETJIT==");
    assert!(!format!("{jit:?}").contains("SECRETJIT"));
    let req = &stub.requests()[0];
    assert_eq!(req.path(), "/repos/o/r/actions/runners/generate-jitconfig");
    assert_eq!(req.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_str(&req.body).expect("body json");
    assert_eq!(
        body,
        json!({"name":"ghm-o-r-abc123","runner_group_id":1,"labels":["self-hosted","macos","arm64"],"work_folder":"_work"})
    );
}

#[test]
fn generate_jit_rejects_foreign_prefix_and_empty_labels_without_a_request() {
    let stub = Stub::start(|_, _| Resp::json(201, &json!({})));
    let c = client(&stub);
    for name in ["gha-o-r-abc123", "ghw-x", "ghm-", "ghm-a/b", ""] {
        assert!(
            matches!(
                c.generate_jit("o/r", name, &["macos".into()]),
                Err(ApiError::Invalid(_))
            ),
            "{name}"
        );
    }
    assert!(matches!(
        c.generate_jit("o/r", "ghm-ok-123456", &[]),
        Err(ApiError::Invalid(_))
    ));
    assert!(stub.requests().is_empty());
}

#[test]
fn generate_jit_conflict_and_validation_errors_are_typed() {
    for code in [409u16, 422] {
        let stub = Stub::start(move |_, _| Resp::json(code, &json!({"message":"x"})));
        let e = client(&stub)
            .generate_jit("o/r", "ghm-o-r-abc123", &["macos".into()])
            .expect_err("err");
        assert_eq!(e, ApiError::Http { status: code });
    }
}

#[test]
fn delete_runner_refuses_non_ghm_registrations_and_does_not_send_delete() {
    let stub = Stub::start(|req, _| match req.method.as_str() {
        "GET" => Resp::json(
            200,
            &json!({"id":9,"name":"gha-linux-abc123","status":"online","busy":false}),
        ),
        _ => Resp::empty(204),
    });
    let err = client(&stub).delete_runner("o/r", 9).expect_err("refused");
    assert!(matches!(err, ApiError::Forbidden(_)), "{err}");
    assert!(stub.requests().iter().all(|r| r.method != "DELETE"));
}

#[test]
fn delete_runner_is_idempotent_on_404_both_before_and_during() {
    let gone = Stub::start(|_, _| Resp::json(404, &json!({"message":"Not Found"})));
    client(&gone).delete_runner("o/r", 9).expect("already gone");
    let race = Stub::start(|req, _| match req.method.as_str() {
        "GET" => Resp::json(200, &json!({"id":9,"name":"ghm-o-r-abc123"})),
        _ => Resp::json(404, &json!({})),
    });
    client(&race)
        .delete_runner("o/r", 9)
        .expect("deleted by someone else");
}

#[test]
fn list_runners_returns_only_ghm_runners_and_maps_state() {
    let stub = Stub::start(|_, _| {
        Resp::json(
            200,
            &json!({"runners":[
                {"id":1,"name":"gha-linux-1","status":"online","busy":true},
                {"id":2,"name":"ghw-win-1","status":"online","busy":false},
                {"id":3,"name":"ghm-o-r-aaaaaa","status":"online","busy":true},
                {"id":4,"name":"ghm-o-r-bbbbbb","status":"offline","busy":false}
            ]}),
        )
    });
    let runners = client(&stub).list_runners("o/r").expect("runners");
    assert_eq!(runners.len(), 2);
    assert!(runners[0].online && runners[0].busy && runners[0].id == 3);
    assert!(!runners[1].online && !runners[1].busy);
}

fn sha_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[test]
fn release_requires_asset_and_checksum() {
    let sha = "a".repeat(64);
    let with = |tag: &str, body: String, assets: serde_json::Value| {
        let stub = Stub::start({
            let (tag, body, assets) = (tag.to_string(), body.clone(), assets.clone());
            move |_, _| Resp::json(200, &json!({"tag_name":tag,"body":body,"assets":assets}))
        });
        client(&stub).latest_runner_release()
    };
    let body = format!("<!-- BEGIN SHA osx-arm64 -->{sha}<!-- END SHA osx-arm64 -->");
    let asset = json!([{"name":"actions-runner-osx-arm64-2.5.0.tar.gz","browser_download_url":"https://github.com/x"}]);
    let ok = with("v2.5.0", body.clone(), asset.clone()).expect("release");
    assert_eq!(
        (ok.version.as_str(), ok.sha256.as_str(), ok.url.as_str()),
        ("2.5.0", sha.as_str(), "https://github.com/x")
    );
    // Missing checksum, missing asset (a linux asset only), malformed version.
    assert!(matches!(
        with("v2.5.0", "no sha".into(), asset.clone()),
        Err(ApiError::Invalid(_))
    ));
    let linux = json!([{"name":"actions-runner-linux-x64-2.5.0.tar.gz","browser_download_url":"https://github.com/y"}]);
    assert!(matches!(
        with("v2.5.0", body.clone(), linux),
        Err(ApiError::Invalid(_))
    ));
    assert!(matches!(
        with("v../x", body, asset),
        Err(ApiError::Invalid(_))
    ));
}

#[test]
fn download_sends_no_token_verifies_sha_and_leaves_no_partial_file() {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let p2 = payload.clone();
    let stub = Stub::start(move |_, _| Resp::bytes(200, p2.clone()));
    let c = client(&stub);
    let dir = std::env::temp_dir().join(format!("mrb-gh-dl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let dest = dir.join("runner.tar.gz");
    let rel = RunnerRelease {
        version: "1.0.0".into(),
        url: format!("{}/asset", stub.url()),
        sha256: sha_hex(&payload),
    };

    c.download_release(&rel, &dest).expect("download");
    assert_eq!(std::fs::read(&dest).expect("read"), payload);
    assert!(!dir.join("runner.tar.gz.part").exists());
    let req = &stub.requests()[0];
    assert!(
        req.header("authorization").is_none(),
        "token must not go to the asset host"
    );
    assert!(!req.header_dump().contains(TOKEN));

    // Uppercase expected digest is accepted; a wrong one is Invalid and leaves nothing.
    let mut upper = rel.clone();
    upper.sha256 = rel.sha256.to_ascii_uppercase();
    std::fs::remove_file(&dest).expect("rm");
    c.download_release(&upper, &dest).expect("uppercase digest");
    std::fs::remove_file(&dest).expect("rm");
    let mut bad = rel.clone();
    bad.sha256 = "0".repeat(64);
    assert!(matches!(
        c.download_release(&bad, &dest),
        Err(ApiError::Invalid(_))
    ));
    assert!(!dest.exists() && !dir.join("runner.tar.gz.part").exists());

    // HTTP error and non-https url (against a non-stub client) leave nothing either.
    let missing = Stub::start(|_, _| Resp::empty(404));
    let mut r404 = rel.clone();
    r404.url = format!("{}/asset", missing.url());
    assert_eq!(
        c.download_release(&r404, &dest),
        Err(ApiError::Http { status: 404 })
    );
    assert!(!dest.exists() && !dir.join("runner.tar.gz.part").exists());
    let prod = GitHubClient::new(Secret::new(TOKEN.into()));
    let mut insecure = rel;
    insecure.url = format!("{}/asset", stub.url());
    assert!(matches!(
        prod.download_release(&insecure, &dest),
        Err(ApiError::Invalid(_))
    ));
    assert!(!dest.exists());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn debug_of_client_shows_no_token() {
    let c = GitHubClient::new(Secret::new(TOKEN.into()));
    let s = format!("{c:?}");
    assert!(s.contains("api.github.com") && !s.contains(TOKEN));
}
