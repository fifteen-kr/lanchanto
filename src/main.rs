// #![deny(warnings)]

use clap::Parser;
use warp::Filter;
use bytes::Bytes;

mod config;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    config: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let _config = config::Config::from_file(&args.config)?;

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let main_page = warp::get().map(|| "Hello, world!");
    
    let github = warp::post()
        .and(warp::path("github"))
        .and(warp::body::bytes())
        .map(|body: Bytes| {
            let text = String::from_utf8_lossy(&body);
            println!("Debug: Received GitHub webhook:\n{}", text);
            "{\"error\":null}"
        });

    println!("Listening on 0.0.0.0:{}", port);
    warp::serve(main_page.or(github)).run(([0, 0, 0, 0], port)).await;

    Ok(())
}
