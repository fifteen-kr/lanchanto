use std::path::{Component, Path};

use anyhow::{bail, Context};
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

    /// Paths (relative to `target`) carried over from the previous deploy: runtime
    /// state the artifact must never clobber (databases, uploads, ...).
    #[serde(default)]
    pub preserve: Vec<String>,
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

        for deploy in &config.deploy {
            for artifact in &deploy.artifact {
                for rel in &artifact.preserve {
                    let is_relative_normal = !rel.is_empty()
                        && Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)));
                    if !is_relative_normal {
                        bail!(
                            "invalid preserve path {:?} for artifact {} of {}: must be a relative path inside the target (no `..`, no absolute or empty paths)",
                            rel, artifact.name, deploy.repository
                        );
                    }
                }
            }
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `contents` to a config file in a fresh tempdir and loads it.
    fn load_from_toml(contents: &str) -> anyhow::Result<Config> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, contents).unwrap();
        Config::load(path)
    }

    /// A minimal config whose single artifact carries the given `preserve` TOML array.
    fn config_with_preserve(preserve: &str) -> String {
        format!(
            r#"
[[deploy]]
repository = "a/b"
branch = "main"

[[deploy.artifact]]
name = "x.zip"
target = "/tmp/x"
preserve = {preserve}
"#
        )
    }

    fn assert_invalid_preserve(preserve: &str) {
        let err = load_from_toml(&config_with_preserve(preserve)).unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid preserve path"),
            "expected a preserve validation error, got: {err:#}"
        );
    }

    #[test]
    fn load_accepts_valid_preserve() {
        let config = load_from_toml(&config_with_preserve(r#"["var", "data/db.sqlite"]"#)).unwrap();

        let artifact = &config.deploy[0].artifact[0];
        assert_eq!(artifact.preserve, ["var", "data/db.sqlite"]);
    }

    #[test]
    fn load_rejects_traversal_preserve() {
        assert_invalid_preserve(r#"["../escape"]"#);
    }

    #[test]
    fn load_rejects_absolute_preserve() {
        assert_invalid_preserve(r#"["/abs"]"#);
    }

    #[test]
    fn load_rejects_empty_preserve_entry() {
        assert_invalid_preserve(r#"[""]"#);
    }
}
