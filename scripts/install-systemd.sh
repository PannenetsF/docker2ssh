#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_BIN="${INSTALL_BIN:-/usr/local/bin/d2s}"
INSTALL_CONFIG_DIR="${INSTALL_CONFIG_DIR:-/etc/d2s}"
INSTALL_UNIT="${INSTALL_UNIT:-/etc/systemd/system/d2s.service}"

if [[ $EUID -ne 0 ]]; then
  echo "please run as root" >&2
  exit 1
fi

mkdir -p "$INSTALL_CONFIG_DIR"

if [[ -f "$ROOT_DIR/target/release/d2s" ]]; then
  install -m 0755 "$ROOT_DIR/target/release/d2s" "$INSTALL_BIN"
else
  echo "missing binary: $ROOT_DIR/target/release/d2s" >&2
  echo "run cargo build --release first" >&2
  exit 1
fi

install -m 0644 "$ROOT_DIR/packaging/systemd/d2s.service" "$INSTALL_UNIT"

if [[ ! -f "$INSTALL_CONFIG_DIR/d2s.env" ]]; then
  install -m 0644 "$ROOT_DIR/packaging/systemd/d2s.env.example" "$INSTALL_CONFIG_DIR/d2s.env"
fi

if [[ ! -f "$INSTALL_CONFIG_DIR/config.toml" ]]; then
  cat >"$INSTALL_CONFIG_DIR/config.toml" <<'EOF'
listen_host = "0.0.0.0"
docker_socket = "/var/run/docker.sock"

[[mappings]]
port = 2222
container = "replace-me"
EOF
fi

systemctl daemon-reload
systemctl enable d2s.service

echo "[d2s] installed binary to $INSTALL_BIN"
echo "[d2s] systemd unit installed to $INSTALL_UNIT"
echo "[d2s] edit $INSTALL_CONFIG_DIR/config.toml and then run:"
echo "  systemctl restart d2s.service"
echo "  systemctl status d2s.service --no-pager"
