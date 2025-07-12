// #![deny(warnings)]

use clap::Parser;
use warp::Filter;

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
    
    let github = warp::get()
        .and(warp::path("github"))
        .map(|| "Hello, world!");

    println!("Listening on 0.0.0.0:{}", port);
    warp::serve(github).run(([0, 0, 0, 0], port)).await;

    Ok(())
}
