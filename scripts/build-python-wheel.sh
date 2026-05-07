#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT_DIR
TARGET="${TARGET:-x86_64-unknown-linux-musl}"
PROFILE="${PROFILE:-release}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
BIN_SRC="${BIN_SRC:-$ROOT_DIR/target/$TARGET/$PROFILE/d2s}"
BIN_DST="$ROOT_DIR/python/src/docker2ssh/bin/d2s"
WHEEL_PLATFORM_TAG="${WHEEL_PLATFORM_TAG:-}"
WHEEL_TAG="${WHEEL_TAG:-}"

cleanup() {
  rm -f "$BIN_DST"
}
trap cleanup EXIT

if [[ ! -f "$BIN_SRC" ]]; then
  echo "[d2s] missing binary: $BIN_SRC" >&2
  echo "[d2s] build it first, e.g. TARGET=$TARGET ./scripts/build-static-linux.sh" >&2
  exit 1
fi

cp "$BIN_SRC" "$BIN_DST"
chmod +x "$BIN_DST"

VENV_DIR="${VENV_DIR:-$ROOT_DIR/.venv-wheel-build}"
"$PYTHON_BIN" -m venv "$VENV_DIR"
"$VENV_DIR/bin/pip" install --upgrade pip build >/dev/null

pushd "$ROOT_DIR/python" >/dev/null
rm -rf dist build

if [[ -n "$WHEEL_TAG" ]]; then
  export D2S_WHEEL_TAG="$WHEEL_TAG"
elif [[ -n "$WHEEL_PLATFORM_TAG" ]]; then
  export D2S_WHEEL_PLATFORM_TAG="$WHEEL_PLATFORM_TAG"
fi

"$VENV_DIR/bin/python" -m build --wheel
popd >/dev/null

echo "[d2s] wheel output:"
ls -1 "$ROOT_DIR/python/dist"

"$VENV_DIR/bin/python" - <<'PY'
from pathlib import Path
import os
import zipfile

wheel = max(Path(os.environ["ROOT_DIR"]).joinpath("python", "dist").glob("*.whl"))
print(f"[d2s] built wheel: {wheel.name}")
with zipfile.ZipFile(wheel) as zf:
    for name in zf.namelist():
        if name.endswith("WHEEL"):
            print("[d2s] wheel metadata:")
            print(zf.read(name).decode().strip())
            break
PY
