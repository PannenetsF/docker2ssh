from __future__ import annotations

import os
import sysconfig
from pathlib import Path

from hatchling.builders.hooks.plugin.interface import BuildHookInterface


def _default_platform_tag() -> str:
    return sysconfig.get_platform().replace("-", "_").replace(".", "_")


class CustomBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict[str, object]) -> None:
        bundled_binary = Path(self.root) / "src" / "docker2ssh" / "bin" / "d2s"
        if not bundled_binary.is_file():
            raise RuntimeError(
                "missing bundled d2s binary at python/src/docker2ssh/bin/d2s; "
                "copy a prebuilt binary first"
            )

        build_data["pure_python"] = False
        build_data["tag"] = os.environ.get(
            "D2S_WHEEL_TAG",
            f"py3-none-{os.environ.get('D2S_WHEEL_PLATFORM_TAG', _default_platform_tag())}",
        )
