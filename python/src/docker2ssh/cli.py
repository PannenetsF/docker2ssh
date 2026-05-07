from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

from .api import run_d2s


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="docker2ssh", add_help=True)
    parser.add_argument("--config", type=Path, default=None, help="Path to d2s config TOML")
    parser.add_argument(
        "args",
        nargs=argparse.REMAINDER,
        help="Arguments passed through to the d2s binary, e.g. show / serve / config set ...",
    )
    return parser


def main() -> int:
    parser = build_parser()
    ns = parser.parse_args()

    forwarded = ns.args
    if forwarded and forwarded[0] == "--":
        forwarded = forwarded[1:]
    if not forwarded:
        forwarded = ["help"]

    proc = run_d2s(forwarded, config=ns.config, check=False, capture_output=False)
    if isinstance(proc, subprocess.CompletedProcess):
        return proc.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
