# Lanĉanto

Lanĉanto is a small program which listens for GitHub webhooks, and deploys projects when it receives a certain event.

This program is currently work-in-progress.

## Usage

```sh
lanchanto --config="config.toml"
```

Configure lanĉanto like this:

```toml
[credential]
# Both may be omitted and provided via the GITHUB_WEBHOOK_SECRET and
# GITHUB_TOKEN environment variables instead.
github_webhook_secret = "..."
github_token = "..."

[[deploy]]
repository = "fifteen-kr/blog"
# Only successful `workflow_run` events for this branch deploy.
# Omitting `branch` deploys ANY branch's successful runs (a warning is logged).
branch = "main"
# Optional: only accept runs of this workflow (matches `workflow_run.name`).
workflow = "Build"

[[deploy.artifact]]
name = "blog.zip"
target = "/var/www/blog"
# Optional: paths (relative to `target`) carried over from the previous deploy —
# runtime state the artifact must never clobber (databases, uploads, ...).
preserve = ["var"]
```

On deploy, each artifact is extracted into a staging directory and swapped into
place, so the target directory is **replaced**, never merged into: files that
vanished from the artifact vanish from the target, and a failed download or
extraction leaves the previous version untouched. Paths listed in `preserve` are
the exception — they are carried over from the previous version after the swap,
replacing any copy of the same path shipped in the artifact (live state wins;
a shipped copy only serves as the seed on first deploy).

Here is an example of a systemd service file:

```ini
[Unit]
Description=Lanchanto
After=network-online.target

[Service]
WorkingDirectory=/home/foo/lanchanto
ExecStart=/home/foo/lanchanto/lanchanto --config="/home/foo/lanchanto-config.toml"
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
```
