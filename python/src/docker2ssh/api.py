from __future__ import annotations

import os
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Optional, Union


class D2SError(RuntimeError):
    pass


def _bundled_binary():
    candidate = Path(__file__).resolve().parent / "bin" / "d2s"
    if candidate.is_file():
        return candidate
    return None


BinaryPath = Union[str, os.PathLike]


def _resolve_binary(binary=None):
    # Prefer an explicitly supplied path, then the bundled binary, then environment/PATH.
    bundled = _bundled_binary()
    candidates = [
        str(binary) if binary else None,
        str(bundled) if bundled else None,
        os.environ.get("D2S_BIN"),
        shutil.which("d2s"),
    ]
    for candidate in candidates:
        if candidate:
            return candidate
    raise D2SError("could not find d2s binary; set D2S_BIN or add d2s to PATH")


def run_d2s(
    args: Iterable[str],
    *,
    binary: Optional[BinaryPath] = None,
    config: Optional[BinaryPath] = None,
    check: bool = True,
    capture_output: bool = True,
    text: bool = True,
) -> subprocess.CompletedProcess:
    cmd = [_resolve_binary(binary)]
    if config is not None:
        cmd.extend(["--config", str(config)])
    cmd.extend(list(args))
    proc = subprocess.run(cmd, check=False, capture_output=capture_output, text=text)
    if check and proc.returncode != 0:
        stderr = (proc.stderr or "").strip()
        stdout = (proc.stdout or "").strip()
        message = stderr or stdout or f"d2s exited with code {proc.returncode}"
        raise D2SError(message)
    return proc


@dataclass
class D2S:
    binary: Optional[Union[str, Path]] = None
    config: Optional[Union[str, Path]] = None

    def run(self, *args: str, check: bool = True) -> subprocess.CompletedProcess:
        return run_d2s(args, binary=self.binary, config=self.config, check=check)

    def help(self) -> str:
        return self.run("help").stdout

    def serve(self, *, check: bool = True) -> subprocess.CompletedProcess[str]:
        return self.run("serve", check=check)

    def show(self) -> str:
        return self.run("show").stdout

    def doctor(
        self,
        *,
        host: str = "127.0.0.1",
        user: str = "docker",
        identity: Optional[Union[str, Path]] = None,
    ) -> str:
        args = ["doctor", "--host", host, "--user", user]
        if identity is not None:
            args.extend(["--identity", str(identity)])
        return self.run(*args).stdout

    def config_set(
        self,
        port: int,
        container: str,
        *,
        shell: Optional[str] = None,
        clear_shell: bool = False,
    ) -> str:
        args = ["config", "set", str(port), container]
        if shell is not None:
            args.extend(["--shell", shell])
        if clear_shell:
            args.append("--clear-shell")
        return self.run(*args).stdout

    def config_rm(self, port: int) -> str:
        return self.run("config", "rm", str(port)).stdout

    def config_list(self) -> str:
        return self.run("config", "list").stdout
