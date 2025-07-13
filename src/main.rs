// #![deny(warnings)]

use std::convert::Infallible;

use once_cell::sync::OnceCell;
use bytes::Bytes;
use hmac::Mac;

use clap::Parser;
use warp::{http::{HeaderMap, StatusCode}, reply::WithStatus, Filter};

mod config;

#[derive(Parser)]
struct Args {
    #[arg(short, long, default_value("8080"))]
    port: u16,

    #[arg(short, long)]
    config: String,
}

static CONFIG: OnceCell<config::Config> = OnceCell::new();

fn init_config(args: &Args) {
    let mut config = config::Config::from_file(&args.config).expect("Failed to load config.");

    if config.credential.githubWebhookSecret.is_empty() {
        config.credential.githubWebhookSecret = std::env::var("GITHUB_WEBHOOK_SECRET").unwrap_or_default();
    }

    if config.credential.githubToken.is_empty() {
        config.credential.githubToken = std::env::var("GITHUB_TOKEN").unwrap_or_default();
    }

    CONFIG.set(config).expect("Failed to set config.");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    init_config(&args);

    let port: u16 = args.port;

    let main_page = warp::get().map(|| "Hello, world!\n");
    
    let github = warp::post()
        .and(warp::path("github"))
        .and(warp::header::headers_cloned())
        .and(warp::body::bytes())
        .and_then(handle_github);

    println!("Listening on 0.0.0.0:{}", port);
    warp::serve(main_page.or(github)).run(([0, 0, 0, 0], port)).await;

    Ok(())
}

async fn handle_github(headers: HeaderMap, body: Bytes) -> Result<impl warp::Reply, Infallible> {
    let config = CONFIG.get().expect("Failed to get config.");

    let event = headers.get("X-GitHub-Event").and_then(|v| v.to_str().ok()).unwrap_or("");
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing webhook body: {}", e);
            return Ok(reply_error(StatusCode::BAD_REQUEST, "invalid body"));
        }
    };

    let repo_full = payload.get("repository").and_then(|v| v.get("full_name")).and_then(|n| n.as_str()).unwrap_or("").to_owned();
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("");

    println!("Hook received: {} {} {}", repo_full, event, action);

    if !verify_signature(&config, &headers, &body) {
        eprintln!("! Error: invalid credential!");
        return Ok(reply_error(StatusCode::FORBIDDEN, "invalid credential"));
    }

    let deploy_conf = match config.deploy.iter().find(|d| d.repository == repo_full) {
        Some(v) => v,
        None => {
            eprintln!("! Error: unknown repository {}", repo_full);
            return Ok(reply_error(StatusCode::BAD_REQUEST, "unknown repository"));
        },
    };

    let artifacts_url = payload.get("workflow_run").and_then(|v| v.get("artifacts_url")).and_then(|v| v.as_str()).unwrap_or("").to_owned();
    let token = config.credential.githubToken.clone();
    tokio::spawn(async move {
        if let Err(e) = download_artifacts(&token, &repo_full, &artifacts_url, &deploy_conf.artifact).await {
            eprintln!("! Failed to download artifacts for {}: {}", repo_full, e);
        }
    });

    Ok(warp::reply::with_status(
        warp::reply::json(&serde_json::json!({"error": null})),
        StatusCode::OK,
    ))
}

fn reply_error(status_code: StatusCode, message: &str) -> WithStatus<warp::reply::Json> {
    warp::reply::with_status(
        warp::reply::json(&serde_json::json!({"error": message})),
        status_code,
    )
}

fn verify_signature(config: &config::Config, headers: &HeaderMap, body: &[u8]) -> bool {
    let secret = config.credential.githubWebhookSecret.as_bytes();
    if secret.is_empty() {
        return false;
    }

    let sig_header =  headers.get("X-Hub-Signature-256").or_else(|| headers.get("X-Hub-Signature")).and_then(|v| v.to_str().ok());
    let sig_header = match sig_header {
        Some(v) => v,
        None => return false,
    };

    let sig = sig_header.strip_prefix("sha256=").unwrap_or("");
    let sig = match hex::decode(sig) {
        Ok(v) => v,
        Err(_) => return false,
    };
    
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;
    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(v) => v,
        Err(_) => return false,
    };
    
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

async fn download_artifacts(token: &str, repo_full: &str, download_url: &str, artifacts: &Vec<config::Artifact>) -> Result<(), Box<dyn std::error::Error>> {
    println!("> Downloading artifacts for {}, url={}", repo_full, download_url);
    let client = reqwest::Client::new();
    Ok(())
}