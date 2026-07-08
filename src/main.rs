use std::convert::Infallible;
use std::sync::OnceLock;

use bytes::Bytes;
use clap::Parser;
use serde::Deserialize;
use warp::{http::{HeaderMap, StatusCode}, reply::WithStatus, Filter};

mod config;
mod signature;
mod download;

/// GitHub caps webhook payloads at 25 MiB, but `workflow_run` payloads are a few tens
/// of KiB; 1 MiB bounds what a client can make us buffer while leaving ample margin.
const MAX_BODY_BYTES: u64 = 1024 * 1024;

#[derive(Parser)]
struct Args {
    #[arg(short, long, default_value_t = 8080)]
    port: u16,

    #[arg(short, long)]
    config: String,
}

/// Process-lifetime home of the loaded config. Written exactly once in `main`;
/// everything else receives the config as a plain reference.
static CONFIG: OnceLock<config::Config> = OnceLock::new();

/// The relevant subset of a `workflow_run` webhook payload.
#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    action: String,
    repository: Repository,
    workflow_run: Option<WorkflowRun>,
}

#[derive(Deserialize)]
struct Repository {
    full_name: String,
}

#[derive(Deserialize)]
struct WorkflowRun {
    conclusion: Option<String>,
    head_branch: Option<String>,
    name: Option<String>,
    artifacts_url: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let loaded = config::Config::load(&args.config)?;
    for deploy in &loaded.deploy {
        if deploy.branch.is_none() {
            eprintln!("! Warning: deploy entry for {} has no `branch` filter; successful runs of ANY branch will deploy.", deploy.repository);
        }
    }
    let config: &'static config::Config = CONFIG.get_or_init(|| loaded);

    let main_page = warp::get().map(|| "Hello, world!\n");

    let github = warp::post()
        .and(warp::path("github"))
        .and(warp::any().map(move || config))
        .and(warp::header::headers_cloned())
        .and(warp::body::content_length_limit(MAX_BODY_BYTES))
        .and(warp::body::bytes())
        .and_then(handle_github);

    println!("Listening on 0.0.0.0:{}", args.port);
    warp::serve(main_page.or(github)).run(([0, 0, 0, 0], args.port)).await;

    Ok(())
}

async fn handle_github(config: &'static config::Config, headers: HeaderMap, body: Bytes) -> Result<impl warp::Reply, Infallible> {
    if let Err(e) = signature::verify(config, &headers, &body) {
        eprintln!("! Error: invalid credential: {}", e);
        return Ok(reply_error(StatusCode::FORBIDDEN, "invalid credential"));
    }

    // Only `workflow_run` carries deployable artifacts. Everything else (`ping`,
    // `check_suite`, ...) is acknowledged and ignored so the hook stays green in
    // GitHub's UI; some of those events also have `action == "completed"`.
    let event = headers.get("X-GitHub-Event").and_then(|v| v.to_str().ok());
    if event != Some("workflow_run") {
        return Ok(reply_ok());
    }

    let payload: Payload = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing webhook body: {}", e);
            return Ok(reply_error(StatusCode::BAD_REQUEST, "invalid body"));
        }
    };

    let repo_full = payload.repository.full_name;
    println!("Hook received: {} workflow_run {}", repo_full, payload.action);

    if payload.action != "completed" {
        return Ok(reply_ok());
    }

    let Some(deploy_conf) = config.deploy.iter().find(|d| d.repository == repo_full) else {
        eprintln!("! Error: unknown repository {}", repo_full);
        return Ok(reply_error(StatusCode::BAD_REQUEST, "unknown repository"));
    };

    let Some(run) = payload.workflow_run else {
        eprintln!("! Error: workflow_run event for {} lacks a workflow_run object", repo_full);
        return Ok(reply_error(StatusCode::BAD_REQUEST, "invalid body"));
    };

    // "completed" is not "succeeded": failed or cancelled runs may still have
    // uploaded artifacts, and those must never deploy.
    if run.conclusion.as_deref() != Some("success") {
        println!("> Ignoring run of {}: conclusion is {:?}.", repo_full, run.conclusion);
        return Ok(reply_ok());
    }

    if let Some(want) = &deploy_conf.branch {
        if run.head_branch.as_deref() != Some(want.as_str()) {
            println!("> Ignoring run of {}: branch {:?} is not {:?}.", repo_full, run.head_branch, want);
            return Ok(reply_ok());
        }
    }

    if let Some(want) = &deploy_conf.workflow {
        if run.name.as_deref() != Some(want.as_str()) {
            println!("> Ignoring run of {}: workflow {:?} is not {:?}.", repo_full, run.name, want);
            return Ok(reply_ok());
        }
    }

    let Some(artifacts_url) = run.artifacts_url.filter(|u| !u.is_empty()) else {
        eprintln!("! Error: missing artifacts_url for {}", repo_full);
        return Ok(reply_error(StatusCode::BAD_REQUEST, "missing artifacts_url"));
    };

    let token = config.credential.github_token.clone();
    tokio::spawn(async move {
        // One deploy at a time per entry: a second run completing mid-deploy would
        // otherwise race extraction into the same target directories.
        let _guard = deploy_conf.lock.lock().await;
        if let Err(e) = download::download_artifacts(&token, &repo_full, &artifacts_url, &deploy_conf.artifact).await {
            eprintln!("! Failed to deploy artifacts for {}: {:#}", repo_full, e);
        }
    });

    Ok(reply_ok())
}

fn reply_ok() -> WithStatus<warp::reply::Json> {
    warp::reply::with_status(
        warp::reply::json(&serde_json::json!({"error": null})),
        StatusCode::OK,
    )
}

fn reply_error(status_code: StatusCode, message: &str) -> WithStatus<warp::reply::Json> {
    warp::reply::with_status(
        warp::reply::json(&serde_json::json!({"error": message})),
        status_code,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    use hmac::{KeyInit, Mac};

    const SECRET: &str = "testsecret";

    /// Shared test config; `&TEST_CONFIG` derefs to the `&'static config::Config`
    /// that `handle_github` expects.
    static TEST_CONFIG: LazyLock<config::Config> = LazyLock::new(|| config::Config {
        credential: config::Credential {
            github_webhook_secret: SECRET.to_owned(),
            github_token: String::new(),
        },
        deploy: vec![config::Deploy {
            repository: "test/repo".to_owned(),
            branch: Some("main".to_owned()),
            workflow: Some("CI".to_owned()),
            artifact: vec![config::Artifact {
                name: "bundle".to_owned(),
                target: "unused".to_owned(),
            }],
            lock: Default::default(),
        }],
    });

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn headers(event: &str, signature: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("X-GitHub-Event", event.parse().unwrap());
        headers.insert("X-Hub-Signature-256", signature.parse().unwrap());
        headers
    }

    fn signed_headers(event: &str, body: &[u8]) -> HeaderMap {
        headers(event, &sign(SECRET, body))
    }

    /// `run` is the `workflow_run` object. Gate tests pass a run satisfying every
    /// gate *behind* the one under test but leave `artifacts_url` out: were the
    /// gate to regress, the request would fall through to the artifacts_url check
    /// and return 400 instead of the gate's 200 — the test fails without ever
    /// reaching the deploy spawn (which also replies 200 and would hide the
    /// regression while hitting the network).
    fn run_payload(action: &str, repo: &str, run: serde_json::Value) -> Vec<u8> {
        serde_json::json!({
            "action": action,
            "repository": { "full_name": repo },
            "workflow_run": run,
        })
        .to_string()
        .into_bytes()
    }

    /// A `workflow_run` object passing every content gate, `artifacts_url` absent.
    fn gate_passing_run() -> serde_json::Value {
        serde_json::json!({
            "conclusion": "success",
            "head_branch": "main",
            "name": "CI",
        })
    }

    async fn status_for(headers: HeaderMap, body: &[u8]) -> StatusCode {
        let reply = handle_github(&TEST_CONFIG, headers, Bytes::copy_from_slice(body)).await.unwrap();
        warp::reply::Reply::into_response(reply).status()
    }

    #[tokio::test]
    async fn bad_signature_is_forbidden() {
        // Otherwise-deployable payload: had verification not run first, this
        // would come back 400 (missing artifacts_url), not 403.
        let body = run_payload("completed", "test/repo", gate_passing_run());
        let headers = headers("workflow_run", &sign("wrongsecret", &body));
        assert_eq!(status_for(headers, &body).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn non_workflow_run_event_acked_before_repo_lookup() {
        // `check_suite` also delivers `action == "completed"` but carries no
        // `workflow_run`; the unknown repository must not matter because the
        // event gate precedes the repo lookup (the old code replied 400 here).
        let body = serde_json::json!({
            "action": "completed",
            "repository": { "full_name": "unknown/repo" },
        })
        .to_string()
        .into_bytes();
        let headers = signed_headers("check_suite", &body);
        assert_eq!(status_for(headers, &body).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_repository_is_bad_request() {
        let body = run_payload("completed", "unknown/repo", gate_passing_run());
        let headers = signed_headers("workflow_run", &body);
        assert_eq!(status_for(headers, &body).await, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn non_completed_action_is_ignored() {
        let body = run_payload("requested", "test/repo", gate_passing_run());
        let headers = signed_headers("workflow_run", &body);
        assert_eq!(status_for(headers, &body).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn failed_conclusion_is_ignored() {
        let mut run = gate_passing_run();
        run["conclusion"] = "failure".into();
        let body = run_payload("completed", "test/repo", run);
        let headers = signed_headers("workflow_run", &body);
        assert_eq!(status_for(headers, &body).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn mismatched_branch_is_ignored() {
        let mut run = gate_passing_run();
        run["head_branch"] = "feature".into();
        let body = run_payload("completed", "test/repo", run);
        let headers = signed_headers("workflow_run", &body);
        assert_eq!(status_for(headers, &body).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn mismatched_workflow_is_ignored() {
        let mut run = gate_passing_run();
        run["name"] = "CodeQL".into();
        let body = run_payload("completed", "test/repo", run);
        let headers = signed_headers("workflow_run", &body);
        assert_eq!(status_for(headers, &body).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_artifacts_url_is_bad_request() {
        // Key absent entirely.
        let body = run_payload("completed", "test/repo", gate_passing_run());
        let headers = signed_headers("workflow_run", &body);
        assert_eq!(status_for(headers, &body).await, StatusCode::BAD_REQUEST);

        // Key present but empty.
        let mut run = gate_passing_run();
        run["artifacts_url"] = "".into();
        let body = run_payload("completed", "test/repo", run);
        let headers = signed_headers("workflow_run", &body);
        assert_eq!(status_for(headers, &body).await, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn malformed_json_is_bad_request() {
        let body = b"{ not json";
        let headers = signed_headers("workflow_run", body);
        assert_eq!(status_for(headers, body).await, StatusCode::BAD_REQUEST);
    }
}
