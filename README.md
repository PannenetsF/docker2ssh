# docker2ssh

[中文说明](./README.zh-CN.md)

`docker2ssh` is a Linux tool for exposing Docker containers through SSH-compatible ports.

It is useful for Cursor, Code-OSS, Trae, and other VS Code-like environments that cannot use the Remote Docker extension because of compliance or platform restrictions.

Typical use cases:

- connect to a specific container with normal SSH tooling
- use `DOCKER_HOST=ssh://...` from local Docker CLI workflows
- support editor workflows that need SSH access or Docker-over-SSH access to a remote container environment

## Install

### Install from PyPI

```bash
python3 -m pip install -U pip
python3 -m pip install -U docker2ssh
```

Notes:

- The published package is for Linux `x86_64` and `aarch64`
- Upgrading `pip` first is recommended on older systems so it recognizes the published wheel tags
- Installing or upgrading the package also installs or updates the bundled `d2s` runtime used by `docker2ssh`

### Build from Source

#### Normal Build

```bash
cargo build --release
```

Binary:

```bash
./target/release/d2s help
```

#### Static Linux Build

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

## Commands

```bash
docker2ssh
docker2ssh help
docker2ssh config set <port> <container>
docker2ssh config rm <port>
docker2ssh config list
docker2ssh show
docker2ssh doctor --host 127.0.0.1 --user docker [--identity /path/to/id_ed25519]
docker2ssh serve
docker2ssh stop
docker2ssh upgrade
```

Notes:

- `docker2ssh` without subcommand prints help.
- `config set` validates that the container exists, even if stopped.
- `show` only prints mappings whose container is currently running.
- `serve` starts a background daemon and writes a pid file next to the config file.
- `stop` stops the daemon started from the same config path.
- `upgrade` runs `python3 -m pip install -U docker2ssh` to update the PyPI package and bundled runtime.

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
- `host_key`: optional SSH host private key path. If missing, the bundled runtime auto-generates one.
- `mappings[].shell`: optional shell override inside the container. If omitted, the bundled runtime tries `/bin/bash`, then `/bin/sh`, then `sh`.

## Typical Usage

Map a port to a container:

```bash
docker2ssh config set 2222 my-app
docker2ssh config set 2223 my-app --shell /bin/bash
```

Start the daemon:

```bash
docker2ssh serve
docker2ssh show
```

From another machine:

```bash
ssh root@your-server -p 2222
ssh root@your-server -p 2222 "ls /"
DOCKER_HOST=ssh://docker@your-server:2222 docker ps
DOCKER_HOST=ssh://docker@your-server:2222 docker cp ./file.txt my-app:/tmp/file.txt
```

Stop it later:

```bash
docker2ssh stop
```

Upgrade docker2ssh from PyPI:

```bash
docker2ssh upgrade
```

If you need a specific Python interpreter:

```bash
PYTHON=/path/to/python docker2ssh upgrade
```

## Daemon Usage

`docker2ssh serve` already detaches into the background, so you can run it directly without an external service manager for the normal case.

Typical lifecycle:

```bash
docker2ssh --config /etc/d2s/config.toml serve
docker2ssh --config /etc/d2s/config.toml stop
```

Notes:

- Access to `/var/run/docker.sock` is required unless you run with a user in the Docker group.
- Binding high ports like `2222` does not require root.
- The daemon pid file is written next to the config file as `d2s.pid`.
- The foreground daemon watches the config file and restarts listeners automatically when mappings change.

## Python Package

This repository also includes the Python packaging used to publish precompiled Linux builds to PyPI.

Package directory:

- `python/`

What it provides:

- a `docker2ssh` console script
- a small Python API that shells out to the `d2s` binary
- `D2S` methods for `serve`, `upgrade`, `show`, `doctor`, `config set/rm/list`
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
- `D2S_BIN` still overrides the bundled binary when you want to use another runtime binary

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
- Python package: `python/`
