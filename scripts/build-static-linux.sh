#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${TARGET:-x86_64-unknown-linux-musl}"
PROFILE="${PROFILE:-release}"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/dist}"

mkdir -p "$OUT_DIR"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required" >&2
  exit 1
fi

echo "[d2s] building static binary for ${TARGET}"
rustup target add "$TARGET"

if [[ "${USE_DOCKER:-0}" == "1" ]]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "docker is required when USE_DOCKER=1" >&2
    exit 1
  fi
  docker run --rm \
    -v "$ROOT_DIR:/work" \
    -w /work \
    clux/muslrust:stable \
    cargo build --profile "$PROFILE" --target "$TARGET"
else
  cargo build --profile "$PROFILE" --target "$TARGET"
fi

BIN_PATH="$ROOT_DIR/target/$TARGET/$PROFILE/d2s"
if [[ ! -f "$BIN_PATH" ]]; then
  echo "binary not found: $BIN_PATH" >&2
  exit 1
fi

cp "$BIN_PATH" "$OUT_DIR/d2s-${TARGET}"
chmod +x "$OUT_DIR/d2s-${TARGET}"

if command -v ldd >/dev/null 2>&1; then
  echo "[d2s] ldd check"
  if ldd "$OUT_DIR/d2s-${TARGET}" 2>&1 | grep -Eq 'not a dynamic executable|statically linked'; then
    echo "[d2s] binary is static"
  else
    echo "[d2s] binary appears to have dynamic dependencies" >&2
    ldd "$OUT_DIR/d2s-${TARGET}" || true
    exit 1
  fi
else
  echo "[d2s] skip ldd check because ldd is unavailable"
fi

echo "[d2s] output: $OUT_DIR/d2s-${TARGET}"
