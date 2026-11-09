//! The real client against a stateful in-process GitHub stub passes the shared contract suites.

mod support;

use std::sync::Mutex;

use macrunnerbar_core::Secret;
use macrunnerbar_github::GitHubClient;
use macrunnerbar_testkit::contract::{self, GitHubFixture};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{Req, Resp, Stub};

pub const ASSET_BYTES: &[u8] = b"pretend this is actions-runner-osx-arm64.tar.gz";

fn sha_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[derive(Default)]
struct World {
    next_id: u64,
    /// (id, name)
    runners: Vec<(u64, String)>,
}

fn github_world() -> Stub {
    let world = Mutex::new(World {
        next_id: 100,
        runners: vec![(7, "gha-linux-1".into())],
    });
    Stub::start(move |req: &Req, base: &str| {
        let mut w = world.lock().expect("world");
        let path = req.path().to_string();
        let seg: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match (req.method.as_str(), seg.as_slice()) {
            ("GET", ["user", "repos"]) => Resp::json(
                200,
                &json!([
                    {"full_name":"owner/repo","private":true,"archived":false,"disabled":false,"permissions":{"admin":true}},
                    {"full_name":"owner/pub","private":false,"archived":false,"permissions":{"admin":true}},
                ]),
            ),
            ("GET", ["repos", "owner", "repo", "actions", "runs"]) => {
                if req.query().contains("status=queued") {
                    Resp::json(
                        200,
                        &json!({"workflow_runs":[{"id":1,"name":"CI","html_url":"https://github.com/owner/repo/actions/runs/1"}]}),
                    )
                } else {
                    Resp::json(200, &json!({"workflow_runs":[]}))
                }
            }
            ("GET", ["repos", "owner", "repo", "actions", "runs", "1", "jobs"]) => Resp::json(
                200,
                &json!({"jobs":[
                    {"id":42,"name":"build","status":"queued","labels":["self-hosted","macOS","arm64"]},
                    {"id":43,"name":"old","status":"completed","labels":["self-hosted"]}
                ]}),
            ),
            (
                "POST",
                [
                    "repos",
                    "owner",
                    "repo",
                    "actions",
                    "runners",
                    "generate-jitconfig",
                ],
            ) => {
                let body: Value = serde_json::from_str(&req.body).expect("json body");
                w.next_id += 1;
                let id = w.next_id;
                let name = body["name"].as_str().expect("name").to_string();
                w.runners.push((id, name.clone()));
                Resp::json(
                    201,
                    &json!({"runner":{"id":id,"name":name,"labels":[{"name":"self-hosted"},{"name":"macos"}]},"encoded_jit_config":"QUJD"}),
                )
            }
            ("GET", ["repos", "owner", "repo", "actions", "runners"]) => {
                let list: Vec<Value> = w
                    .runners
                    .iter()
                    .map(|(id, n)| json!({"id":id,"name":n,"status":"offline","busy":false}))
                    .collect();
                Resp::json(200, &json!({"total_count":list.len(),"runners":list}))
            }
            ("GET" | "DELETE", ["repos", "owner", "repo", "actions", "runners", id]) => {
                let id: u64 = id.parse().expect("id");
                let found = w.runners.iter().find(|(i, _)| *i == id).cloned();
                match (req.method.as_str(), found) {
                    (_, None) => Resp::json(404, &json!({"message":"Not Found"})),
                    ("GET", Some((i, n))) => Resp::json(
                        200,
                        &json!({"id":i,"name":n,"status":"offline","busy":false}),
                    ),
                    (_, Some((i, _))) => {
                        w.runners.retain(|(x, _)| *x != i);
                        Resp::empty(204)
                    }
                }
            }
            ("GET", ["repos", "actions", "runner", "releases", "latest"]) => Resp::json(
                200,
                &json!({
                    "tag_name":"v2.999.1",
                    "body": format!("- actions-runner-osx-arm64-2.999.1.tar.gz <!-- BEGIN SHA osx-arm64 -->{}<!-- END SHA osx-arm64 -->", sha_hex(ASSET_BYTES)),
                    "assets":[{"name":"actions-runner-osx-arm64-2.999.1.tar.gz","browser_download_url":format!("{base}/dl/runner.tar.gz")}]
                }),
            ),
            ("GET", ["dl", "runner.tar.gz"]) => Resp::bytes(200, ASSET_BYTES.to_vec()),
            ("GET", ["repos", "owner", "missing", ..]) => {
                Resp::json(404, &json!({"message":"Not Found"}))
            }
            _ => Resp::json(404, &json!({"message":"Not Found"})),
        }
    })
}

fn client(stub: &Stub) -> GitHubClient {
    GitHubClient::with_base_url(Secret::new("ghp_CONTRACT_TOKEN".into()), stub.url())
}

fn fixture() -> GitHubFixture {
    GitHubFixture {
        repo: "owner/repo".into(),
        queued_job_id: 42,
        unknown_repo: "owner/missing".into(),
        labels: vec!["self-hosted".into(), "macos".into(), "arm64".into()],
    }
}

#[test]
fn client_passes_github_contract() {
    let stub = github_world();
    let c = client(&stub);
    contract::github_contract(&c, &fixture());
    // The foreign `gha-` runner is neither listed nor touched.
    assert!(
        !stub
            .requests()
            .iter()
            .any(|r| r.method == "DELETE" && r.path().ends_with("/runners/7"))
    );
}

#[test]
fn client_passes_download_contract() {
    let stub = github_world();
    let c = client(&stub);
    let release = macrunnerbar_core::GitHubApi::latest_runner_release(&c).expect("release");
    assert_eq!(release.version, "2.999.1");
    contract::github_download_contract(&c, &release);
}
