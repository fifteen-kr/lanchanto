#![deny(warnings)]

use warp::Filter;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    
    let github = warp::get()
        .and(warp::path("github"))
        .map(|| "Hello, world!");

    println!("Listening on 0.0.0.0:{}", port);
    warp::serve(github).run(([0, 0, 0, 0], port)).await;
}
