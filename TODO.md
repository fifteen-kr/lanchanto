# TODO

## Tier 1

- Deploy hooks: per-deploy `pre_deploy` / `post_deploy` commands (e.g. `systemctl --user restart ...`) — required for anything that isn't statically served files.
- Release history + rollback.
  - Extract into `releases/<run_id>`, flip a `current` symlink, keep last N.
  - `lanchanto rollback <repo>` (subsumes the current two-rename swap).

## Tier 2

- Discord webhook notification on deploy success/failure (repo, branch, short SHA, duration).
- Report deployment/commit status back to GitHub using the existing token.
- Manual operations.
  - Authenticated `POST /deploy/<repo>`: redeploy from the latest successful run (recovery without pushing a commit).
  - `GET /status`: last deploy per repo (run id, commit, timestamp, outcome).
- Per-repo credentials: fine-grained token + webhook secret per `[[deploy]]` entry instead of one global pair.
