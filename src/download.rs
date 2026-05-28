use std::{collections::HashMap, path::{Path, PathBuf}};
use std::fs::{self, File};
use std::io::{self, Cursor};

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

pub async fn download_artifacts(token: &str, repo_full: &str, download_url: &str, artifacts: &[config::Artifact]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("> Fetching artifacts for {}, url={}", repo_full, download_url);

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

    let artifact_map: HashMap<_, _> = artifact_list
        .artifacts
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect();

    for wanted in artifacts {
        if let Some(entry) = artifact_map.get(wanted.name.as_str()) {
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

            let target_path = PathBuf::from(&wanted.target);
            tokio::task::spawn_blocking(move || unzip_bytes(&bytes, &target_path)).await??;
        }
    }

    println!("> Downloaded all artifacts for {} successfully!", repo_full);
    Ok(())
}

fn unzip_bytes(data: &[u8], target: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    fs::create_dir_all(target)?;

    let reader = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let out_path = match file.enclosed_name() {
            Some(p) => target.join(p),
            None => continue,
        };

        if file.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }

            let mut out_file = File::create(&out_path)?;
            io::copy(&mut file, &mut out_file)?;
        }
    }

    Ok(())
}