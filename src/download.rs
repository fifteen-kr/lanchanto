use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Seek};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context};
use tokio::io::AsyncWriteExt;

use crate::config;

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

/// Shared client so timeouts are enforced uniformly: reqwest has NO default timeouts,
/// and a stalled connection would otherwise pin its deploy task forever. No total
/// request timeout on purpose — artifact downloads may legitimately take minutes;
/// `read_timeout` catches stalls without capping size.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("lanchanto")
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client.")
});

pub async fn download_artifacts(token: &str, repo_full: &str, download_url: &str, artifacts: &[config::Artifact]) -> anyhow::Result<()> {
    println!("> Fetching artifacts for {}, url={}", repo_full, download_url);

    ensure!(!token.is_empty(), "empty github token");

    // The default page size is 30; ask for the maximum so a run with many artifacts
    // doesn't hide the wanted ones on a later page.
    let list_url = format!("{download_url}?per_page=100");
    let artifact_list: ArtifactList = CLIENT
        .get(&list_url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("failed to list workflow artifacts")?;

    let artifact_map: HashMap<&str, &ArtifactEntry> = artifact_list
        .artifacts
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect();

    // All-or-nothing: deploying a subset of the configured artifacts would leave a
    // mixed-version deployment, so fail before touching any target.
    let mut matched = Vec::with_capacity(artifacts.len());
    let mut missing = Vec::new();
    for wanted in artifacts {
        match artifact_map.get(wanted.name.as_str()) {
            Some(&entry) => matched.push((wanted, entry)),
            None => missing.push(wanted.name.as_str()),
        }
    }
    ensure!(missing.is_empty(), "run has no artifact(s) named: {}", missing.join(", "));

    for (wanted, entry) in matched {
        println!("> Downloading {} to {}...", entry.name, wanted.target);

        let zip_file = fetch_to_temp_file(&entry.archive_download_url, token)
            .await
            .with_context(|| format!("failed to download artifact {}", entry.name))?;

        let target_path = PathBuf::from(&wanted.target);
        let preserve = wanted.preserve.clone();
        tokio::task::spawn_blocking(move || deploy_zip(zip_file, &target_path, &preserve))
            .await
            .context("deploy task panicked")?
            .with_context(|| format!("failed to deploy artifact {}", entry.name))?;
    }

    println!("> Deployed all artifacts for {} successfully!", repo_full);
    Ok(())
}

/// Streams the artifact archive into an unnamed temp file (reclaimed by the OS even if
/// we crash) instead of buffering it in memory; artifacts can be hundreds of megabytes.
async fn fetch_to_temp_file(url: &str, token: &str) -> anyhow::Result<File> {
    let mut response = CLIENT
        .get(url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?;

    let mut file = tokio::fs::File::from_std(tempfile::tempfile()?);
    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
    }

    let mut file = file.into_std().await;
    file.rewind()?;
    Ok(file)
}

/// Extracts into a staging directory next to `target`, then swaps it in. The live
/// directory is never unzipped over: a failed download or extraction leaves it
/// untouched, and files removed upstream don't linger from previous deploys.
fn deploy_zip(zip_file: File, target: &Path, preserve: &[String]) -> anyhow::Result<()> {
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .with_context(|| format!("target {} has no parent directory", target.display()))?;
    let name = target
        .file_name()
        .with_context(|| format!("target {} has no directory name", target.display()))?
        .to_string_lossy();

    fs::create_dir_all(parent)?;

    // Staging lives next to the target so the swap renames stay on one filesystem.
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let staging = parent.join(format!(".{name}.new-{millis}"));
    let old = parent.join(format!(".{name}.old-{millis}"));

    unzip_to(zip_file, &staging)
        .and_then(|()| swap_dirs(&staging, target, &old, preserve))
        .inspect_err(|_| {
            let _ = fs::remove_dir_all(&staging);
        })
}

fn unzip_to(zip_file: File, staging: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(staging)?;

    let mut archive = zip::ZipArchive::new(zip_file)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        // `enclosed_name` rejects absolute paths and `..` traversal (zip-slip).
        let Some(enclosed) = file.enclosed_name() else {
            continue;
        };
        let out_path = staging.join(enclosed);

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

/// Replaces `target` with `staging`: rename the live directory to `old`, rename
/// `staging` into place, carry `preserve` paths over from `old`, then delete `old`.
/// Not a single atomic step, but the vulnerable window is a few renames instead of
/// the whole extraction — on swap failure the previous version is renamed back, and
/// on carry failure `old` is kept on disk so no preserved state is ever lost.
fn swap_dirs(staging: &Path, target: &Path, old: &Path, preserve: &[String]) -> anyhow::Result<()> {
    if !target.exists() {
        // First deploy: nothing to carry; artifact-shipped copies of preserved
        // paths (if any) stay as the initial state.
        fs::rename(staging, target)?;
        return Ok(());
    }

    fs::rename(target, old)?;
    if let Err(e) = fs::rename(staging, target) {
        return Err(match fs::rename(old, target) {
            Ok(()) => anyhow::Error::new(e).context("failed to swap in new version; previous version restored"),
            Err(e2) => anyhow::Error::new(e).context(format!(
                "failed to swap in new version; RESTORE FAILED ({e2}), previous version left at {}",
                old.display()
            )),
        });
    }

    carry_preserved(old, target, preserve).with_context(|| {
        format!(
            "deployed, but carrying preserved paths failed; previous version kept at {}",
            old.display()
        )
    })?;

    if let Err(e) = fs::remove_dir_all(old) {
        // The new version is live; a leftover old tree is cosmetic. Don't fail the deploy.
        eprintln!("! Warning: failed to remove previous version at {}: {}", old.display(), e);
    }
    Ok(())
}

/// Moves each `preserve` path (runtime state the artifact must not clobber) from the
/// previous version into the freshly deployed target. Live state wins: the copy of a
/// path shipped in the artifact is discarded first. Same-filesystem renames, so cheap.
fn carry_preserved(old: &Path, target: &Path, preserve: &[String]) -> anyhow::Result<()> {
    for rel in preserve {
        let from = old.join(rel);
        if !from.exists() {
            continue;
        }

        let to = target.join(rel);
        if to.is_dir() {
            fs::remove_dir_all(&to).with_context(|| format!("failed to drop shipped copy of {rel}"))?;
        } else if to.exists() {
            fs::remove_file(&to).with_context(|| format!("failed to drop shipped copy of {rel}"))?;
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&from, &to).with_context(|| format!("failed to carry preserved path {rel}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a zip archive in an unnamed temp file, rewound and ready to read.
    /// `Some(contents)` adds a file entry, `None` a directory entry.
    fn build_zip(entries: &[(&str, Option<&str>)]) -> File {
        let mut writer = zip::ZipWriter::new(tempfile::tempfile().unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for &(name, contents) in entries {
            match contents {
                // `start_file` (unlike `start_file_from_path`) keeps the raw name, so
                // hostile entries like `../evil.txt` reach the extractor unsanitized.
                Some(contents) => {
                    writer.start_file(name, options).unwrap();
                    writer.write_all(contents.as_bytes()).unwrap();
                }
                None => {
                    writer.add_directory(name, options).unwrap();
                }
            }
        }
        let mut file = writer.finish().unwrap();
        file.rewind().unwrap();
        file
    }

    fn read_file(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {}", path.display(), e))
    }

    /// Sorted names of the direct children of `dir`.
    fn dir_entry_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn fresh_deploy_creates_target_with_exact_zip_contents() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        let zip = build_zip(&[
            ("hello.txt", Some("hello world")),
            ("sub", None),
            ("sub/inner.txt", Some("nested contents")),
        ]);

        deploy_zip(zip, &target, &[]).unwrap();

        assert_eq!(read_file(&target.join("hello.txt")), "hello world");
        assert_eq!(read_file(&target.join("sub").join("inner.txt")), "nested contents");
        assert_eq!(dir_entry_names(&target), ["hello.txt", "sub"]);
        assert_eq!(dir_entry_names(&target.join("sub")), ["inner.txt"]);
    }

    #[test]
    fn deploy_replaces_target_instead_of_merging() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("stale.txt"), "left over from a previous deploy").unwrap();
        fs::write(target.join("common.txt"), "old contents").unwrap();

        let zip = build_zip(&[("common.txt", Some("new contents"))]);
        deploy_zip(zip, &target, &[]).unwrap();

        assert!(
            !target.join("stale.txt").exists(),
            "file absent from the new artifact must not survive the deploy (merge instead of replace)"
        );
        assert_eq!(read_file(&target.join("common.txt")), "new contents");
        assert_eq!(dir_entry_names(&target), ["common.txt"]);
    }

    #[test]
    fn successful_deploy_leaves_no_staging_or_old_debris() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("v1.txt"), "v1").unwrap();

        let zip = build_zip(&[("v2.txt", Some("v2"))]);
        deploy_zip(zip, &target, &[]).unwrap();

        // Both the `.app.new-*` staging dir and the `.app.old-*` renamed previous
        // version must be gone; the parent holds only the live target.
        assert_eq!(dir_entry_names(dir.path()), ["app"]);
        assert_eq!(read_file(&target.join("v2.txt")), "v2");
    }

    #[test]
    fn hostile_zip_entries_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        let zip = build_zip(&[
            ("../evil.txt", Some("escaped via parent traversal")),
            ("/abs_evil.txt", Some("escaped via absolute path")),
            ("safe.txt", Some("safe contents")),
        ]);

        deploy_zip(zip, &target, &[]).unwrap();

        // The invariant is containment: nothing may land outside the staging dir.
        // zip >= 8 `enclosed_name` skips `..`-underflow entries entirely, but
        // NORMALIZES absolute names to relative ones ("similar to other ZIP
        // tools"), so `/abs_evil.txt` deploys contained as `<target>/abs_evil.txt`.
        // Staging lives at `<parent>/.app.new-*`, so `../evil.txt` would land at
        // `<parent>/evil.txt` if traversal were honored.
        assert!(!dir.path().join("evil.txt").exists(), "traversal entry escaped the staging dir");
        assert_eq!(
            dir_entry_names(&target),
            ["abs_evil.txt", "safe.txt"],
            "traversal entry skipped, absolute entry contained, safe entry deployed"
        );
        assert_eq!(read_file(&target.join("safe.txt")), "safe contents");
        assert_eq!(read_file(&target.join("abs_evil.txt")), "escaped via absolute path");
        assert_eq!(dir_entry_names(dir.path()), ["app"], "parent holds only the live target");
    }

    #[test]
    fn corrupt_zip_leaves_live_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), "precious").unwrap();

        let mut garbage = tempfile::tempfile().unwrap();
        garbage.write_all(b"this is not a zip archive").unwrap();
        garbage.rewind().unwrap();

        let result = deploy_zip(garbage, &target, &[]);

        assert!(result.is_err(), "corrupt archive must fail the deploy");
        assert_eq!(read_file(&target.join("keep.txt")), "precious");
        assert_eq!(dir_entry_names(&target), ["keep.txt"]);
        assert_eq!(
            dir_entry_names(dir.path()),
            ["app"],
            "failed deploy must clean up its staging dir and leave no debris"
        );
    }

    #[test]
    fn preserved_file_survives_deploy() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        fs::create_dir_all(target.join("var")).unwrap();
        fs::write(target.join("var").join("db.sqlite"), "precious rows").unwrap();
        fs::write(target.join("stale.txt"), "not in the new artifact").unwrap();

        let zip = build_zip(&[("index.html", Some("<html>v2</html>"))]);
        deploy_zip(zip, &target, &["var".to_string()]).unwrap();

        assert_eq!(read_file(&target.join("var").join("db.sqlite")), "precious rows");
        assert!(
            !target.join("stale.txt").exists(),
            "non-preserved file must not survive the swap"
        );
        assert_eq!(read_file(&target.join("index.html")), "<html>v2</html>");
        assert_eq!(dir_entry_names(&target), ["index.html", "var"]);
        assert_eq!(dir_entry_names(&target.join("var")), ["db.sqlite"]);
        assert_eq!(dir_entry_names(dir.path()), ["app"], "no .old-*/.new-* debris in the parent");
    }

    #[test]
    fn live_state_wins_over_shipped_copy() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        fs::create_dir_all(target.join("var")).unwrap();
        fs::write(target.join("var").join("db.sqlite"), "precious rows").unwrap();

        // The artifact ships its own copy of the preserved dir; the live one must
        // replace it wholesale, not merge with it.
        let zip = build_zip(&[("var/seed.txt", Some("factory seed"))]);
        deploy_zip(zip, &target, &["var".to_string()]).unwrap();

        assert_eq!(
            dir_entry_names(&target.join("var")),
            ["db.sqlite"],
            "shipped var/ must be discarded wholesale, not merged with live state"
        );
        assert_eq!(read_file(&target.join("var").join("db.sqlite")), "precious rows");
    }

    #[test]
    fn nested_preserve_path_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        fs::create_dir_all(target.join("var")).unwrap();
        fs::write(target.join("var").join("data.db"), "precious rows").unwrap();
        fs::write(target.join("var").join("cache.tmp"), "rebuildable").unwrap();

        // The zip ships no var/ at all, so carrying var/data.db must create the
        // parent directory inside the new target.
        let zip = build_zip(&[("index.html", Some("<html>v2</html>"))]);
        deploy_zip(zip, &target, &["var/data.db".to_string()]).unwrap();

        assert_eq!(read_file(&target.join("var").join("data.db")), "precious rows");
        assert!(
            !target.join("var").join("cache.tmp").exists(),
            "only the named path is preserved, not its siblings"
        );
        assert_eq!(dir_entry_names(&target), ["index.html", "var"]);
        assert_eq!(dir_entry_names(&target.join("var")), ["data.db"]);
    }

    #[test]
    fn missing_preserve_path_is_noop_and_seed_stays() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("v1.txt"), "v1").unwrap();

        // Nothing to carry: the live target never grew a var/. The copy shipped
        // in the artifact stays as the initial state.
        let zip = build_zip(&[("var/seed.txt", Some("factory seed"))]);
        deploy_zip(zip, &target, &["var".to_string()]).unwrap();

        assert_eq!(read_file(&target.join("var").join("seed.txt")), "factory seed");
        assert_eq!(dir_entry_names(&target.join("var")), ["seed.txt"]);
        assert_eq!(dir_entry_names(&target), ["var"]);
    }

    #[test]
    fn first_deploy_keeps_shipped_seed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");

        // No previous version: preserve has nothing to carry and must not
        // interfere with the shipped seed.
        let zip = build_zip(&[("var/seed.txt", Some("factory seed"))]);
        deploy_zip(zip, &target, &["var".to_string()]).unwrap();

        assert_eq!(read_file(&target.join("var").join("seed.txt")), "factory seed");
        assert_eq!(dir_entry_names(&target.join("var")), ["seed.txt"]);
        assert_eq!(dir_entry_names(dir.path()), ["app"]);
    }
}
