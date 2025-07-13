use std::path::Path;

use super::config;

#[derive(serde::Deserialize)]
struct ArtifactEntry {
    name: String,
    archive_download_url: String,
}

#[derive(serde::Deserialize)]
struct ArtifactList {
    #[serde(default)]
    artifacts: Vec<ArtifactEntry>,
}

pub async fn download_artifacts(token: &str, repo_full: &str, download_url: &str, artifacts: &Vec<config::Artifact>) -> Result<(), Box<dyn std::error::Error>> {
    println!("> Downloading artifacts for {}, url={}", repo_full, download_url);

    if token.is_empty() {
        return Err("empty github token".into());
    }

    let client = reqwest::Client::new();
    let artifact_list: ArtifactList = client
       .get(download_url)
       .bearer_auth(token)
       .header("User-Agent", "lanchanto")
       .send()
       .await?
       .error_for_status()?
       .json()
       .await?;

    // O(n^2) loop, but it's fine for now
    for wanted in artifacts {
        if let Some(entry) = artifact_list.artifacts.iter().find(|entry| entry.name == wanted.name) {
            println!("> Downloading {} to {}...", entry.name, wanted.target);
            
            let bytes = client
                .get(&entry.archive_download_url)
                .bearer_auth(token)
                .header("User-Agent", "lanchanto")
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;

            unzip_bytes(&bytes, Path::new(&wanted.target)).await?;
        }
    }

    Ok(())
}

async fn unzip_bytes(data: &[u8], target: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // TODO

    Ok(())
}