# Lanĉanto

Lanĉanto is a small program which listens for GitHub webhooks, and deploys projects when it receives a certain event.

This program is currently work-in-progress.

## Usage

```sh
lanchanto --config="config.toml"
```

Configure lanĉanto like this:

```toml
[[deploy]]
repository = "fifteen-kr/blog"

[[deploy.artifact]]
name = "blog.zip"
target = "/var/www/blog"
```

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
