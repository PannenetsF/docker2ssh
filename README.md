# d2s

`d2s` (`docker2ssh`) is a Rust service for Linux servers.

It exposes SSH-compatible ports. Each port is bound to one Docker container and supports two access modes:

- normal SSH access, mapped to `docker exec`
- Docker CLI remote access, mapped to `docker system dial-stdio`

This makes clients such as:

- `docker ps`
- `docker inspect`
- `docker cp`
- `docker exec`

work through:

```bash
DOCKER_HOST=ssh://docker@your-server:2222 docker ps
```

Each SSH port maps to one Docker container reference.

## What It Does

- Exposes one SSH listener per configured port.
- Accepts normal SSH shell / exec requests and maps them to `docker exec`.
- Accepts Docker CLI's `docker system dial-stdio` exec request.
- Proxies Docker HTTP API traffic to the host Docker socket.
- Restricts `/containers/<id or name>/...` requests to the mapped container.
- Allows container references by name, full ID, or short ID.
- Supports stopped containers in config storage.
- Shows only running containers in `show`.
- Verifies active mappings with `doctor`.

## Build

### Normal Build

```bash
cargo build --release
```

Binary:

```bash
./target/release/d2s help
```

### Static Linux Build

For a Linux binary without external `.so` dependencies, build against `musl`:

```bash
./scripts/build-static-linux.sh
```

Default output:

```bash
./dist/d2s-x86_64-unknown-linux-musl
```

Notes:

- Default target: `x86_64-unknown-linux-musl`
- Override target with `TARGET=aarch64-unknown-linux-musl`
- Use `USE_DOCKER=1` to build inside `clux/muslrust:stable`
- The script verifies the output with `ldd` when available

## Commands

```bash
d2s
d2s help
d2s config set <port> <container>
d2s config rm <port>
d2s config list
d2s show
d2s doctor --host 127.0.0.1 --user docker [--identity /path/to/id_ed25519]
d2s serve
```

Notes:

- `d2s` without subcommand is the same as `d2s serve`.
- `config set` validates that the container exists, even if stopped.
- `show` only prints mappings whose container is currently running.

## Config File

Default path:

- Linux: `~/.config/d2s/config.toml`

Example:

```toml
listen_host = "0.0.0.0"
docker_socket = "/var/run/docker.sock"
authorized_keys = "/etc/d2s/authorized_keys"
host_key = "/etc/d2s/host_key_ed25519"

[[mappings]]
port = 2222
container = "my-app"
shell = "/bin/bash"

[[mappings]]
port = 2223
container = "3d2c1b0a9f87"
```

Fields:

- `listen_host`: bind address for SSH listeners.
- `docker_socket`: Docker daemon Unix socket path.
- `authorized_keys`: optional OpenSSH `authorized_keys` file. If omitted, server accepts unauthenticated test connections. Do not use that in production.
- `host_key`: optional SSH host private key path. If missing, `d2s` auto-generates one.
- `mappings[].shell`: optional shell override inside the container. If omitted, `d2s` tries `/bin/bash`, then `/bin/sh`, then `sh`.

## Typical Usage

Map a port to a container:

```bash
d2s config set 2222 my-app
d2s config set 2223 my-app --shell /bin/bash
```

Start the service:

```bash
d2s serve
```

From another machine:

```bash
ssh root@your-server -p 2222
ssh root@your-server -p 2222 "ls /"
DOCKER_HOST=ssh://docker@your-server:2222 docker ps
DOCKER_HOST=ssh://docker@your-server:2222 docker cp ./file.txt my-app:/tmp/file.txt
```

## Linux Deployment

Example `systemd` unit:

```ini
[Unit]
Description=d2s docker2ssh
Documentation=https://code.byted.org/fanyunqian.1/docker2ssh
After=network-online.target docker.service
Wants=network-online.target
Requires=docker.service

[Service]
Type=simple
User=root
Group=root
EnvironmentFile=-/etc/d2s/d2s.env
ExecStart=/usr/local/bin/d2s --config /etc/d2s/config.toml serve
Restart=always
RestartSec=2s

[Install]
WantedBy=multi-user.target
```

Files included in this repository:

- Unit file: `packaging/systemd/d2s.service`
- Env example: `packaging/systemd/d2s.env.example`
- Install script: `scripts/install-systemd.sh`

Quick install:

```bash
cargo build --release
sudo ./scripts/install-systemd.sh
sudo systemctl restart d2s.service
sudo systemctl status d2s.service --no-pager
```

Why `root`:

- Access to `/var/run/docker.sock` is required unless you run with a user in the Docker group.
- Binding high ports like `2222` does not require root.

## Python Package

This repository also includes a wheel-only Python package for publishing precompiled
Linux builds to PyPI.

Package directory:

- `python/`

What it provides:

- a `docker2ssh` console script
- a small Python API that shells out to the `d2s` binary
- `D2S` methods for `serve`, `show`, `doctor`, `config set/rm/list`
- a bundled `d2s` binary inside each published wheel

Build a precompiled wheel after producing the static binary:

```bash
TARGET=x86_64-unknown-linux-musl ./scripts/build-static-linux.sh
TARGET=x86_64-unknown-linux-musl ./scripts/build-python-wheel.sh
```

Notes:

- The wheel metadata is emitted as `py3-none-<platform>`
- CI builds Linux `x86_64` and `aarch64` wheels in `.github/workflows/python-wheels.yml`
- Tag `vX.Y.Z` publishes package version `X.Y.Z`; if that version already exists on PyPI, CI automatically uses `X.Y.Z.postN`
- No source distribution is required for installs from published wheels
- `D2S_BIN` still overrides the bundled binary when you want to use another `d2s`

If you want to point the wrapper at a non-standard binary path:

```bash
export D2S_BIN=/usr/local/bin/d2s
```

Example:

```python
from docker2ssh import D2S

client = D2S(config="/etc/d2s/config.toml")
print(client.show())
client.config_set(2222, "my-container")
```

## Security Model

- Authentication is SSH public-key based when `authorized_keys` is configured.
- Normal SSH access is translated to `docker exec`.
- Docker CLI's `docker system dial-stdio` is still accepted for remote Docker API access.
- Requests targeting `/containers/<ref>/...` are rewritten and restricted to the mapped container.
- Global Docker endpoints like `/_ping`, `/version`, `/info`, `/events` are still allowed.

This is a pragmatic container-scoped proxy, not a complete Docker RBAC implementation.

## Tests

Run:

```bash
cargo test
```

Current test coverage includes:

- config read/write and upsert/remove behavior
- Docker path authorization and rewriting
- HTTP proxy over Unix socket
- SSH end-to-end doctor flow over a mock Docker backend
- SSH shell mode mapped to container exec
- SSH exec mode mapped to container exec

## Repo Layout

- Rust server: `src/`
- Static build script: `scripts/build-static-linux.sh`
- systemd assets: `packaging/systemd/`
- Python package: `python/`
