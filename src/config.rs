use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

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

    /// Only deploy runs on this branch (`workflow_run.head_branch`).
    /// Unset = any branch deploys; a startup warning is emitted.
    pub branch: Option<String>,

    /// Only deploy runs of the workflow with this name (`workflow_run.name`).
    /// Unset = any workflow with matching artifacts deploys.
    pub workflow: Option<String>,

    #[serde(default)]
    pub artifact: Vec<Artifact>,

    /// Serializes deploys of this entry; two runs completing back-to-back must not
    /// race extraction into the same target directories.
    #[serde(skip)]
    pub lock: tokio::sync::Mutex<()>,
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
    /// Loads the config file, filling unset credentials from the environment
    /// (`GITHUB_WEBHOOK_SECRET`, `GITHUB_TOKEN`).
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let mut config: Self = toml::from_str(&data)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;

        let credential = &mut config.credential;
        if credential.github_webhook_secret.is_empty() {
            if let Ok(secret) = std::env::var("GITHUB_WEBHOOK_SECRET") {
                credential.github_webhook_secret = secret;
            }
        }
        if credential.github_token.is_empty() {
            if let Ok(token) = std::env::var("GITHUB_TOKEN") {
                credential.github_token = token;
            }
        }

        Ok(config)
    }
}
