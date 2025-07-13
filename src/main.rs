// #![deny(warnings)]

use std::convert::Infallible;

use clap::Parser;
use warp::{http::{HeaderMap, StatusCode}, reply::WithStatus, Filter};
use bytes::Bytes;
use once_cell::sync::OnceCell;

mod config;

static CONFIG: OnceCell<config::Config> = OnceCell::new();

#[derive(Parser)]
struct Args {
    #[arg(long)]
    config: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let config = config::Config::from_file(&args.config)?;
    CONFIG.set(config).expect("Failed to set config.");

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

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

    let repo_full = payload.get("repository").and_then(|v| v.get("full_name")).and_then(|n| n.as_str()).unwrap_or("");
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("");

    println!("Hook received: {} {} {}", repo_full, event, action);

    if !verify_signature(&headers, &body) {
        eprintln!("Error: invalid credential!");
        return Ok(reply_error(StatusCode::FORBIDDEN, "invalid credential"));
    }

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

fn verify_signature(headers: &HeaderMap, body: &[u8]) -> bool {
    // TODO: implement signature verification
    return false
}