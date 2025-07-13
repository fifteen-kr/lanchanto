use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub credential: Credential,

    #[serde(default)]
    pub deploy: Vec<Deploy>,
}

#[derive(Debug, Deserialize)]
pub struct Deploy {
    pub repository: String,
    #[serde(default)]
    pub artifact: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct Credential {
    #[serde(default)]
    pub github_webhook_secret: String,
    
    #[serde(default)]
    pub github_token: String,
}

impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let data = fs::read_to_string(path)?;
        let config = toml::from_str(&data)?;
        Ok(config)
    }
}