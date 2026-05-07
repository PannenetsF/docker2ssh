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

```bash
cargo build --release
```

Binary:

```bash
./target/release/d2s help
```

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
After=network.target docker.service
Requires=docker.service

[Service]
User=root
Group=root
ExecStart=/usr/local/bin/d2s serve
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
```

Why `root`:

- Access to `/var/run/docker.sock` is required unless you run with a user in the Docker group.
- Binding high ports like `2222` does not require root.

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
