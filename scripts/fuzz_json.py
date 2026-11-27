#!/usr/bin/env python3
"""Bounded JSON/HTTP wire encode fuzz in super-host (see host_net::json_encode_fuzz_timed)."""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    secs = os.environ.get("SUPER_JSON_FUZZ_SECS", "45")
    env = os.environ.copy()
    env["SUPER_JSON_FUZZ_SECS"] = secs
    return subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "super-host",
            "host_net::tests::json_encode_fuzz_timed",
            "--lib",
            "--",
            "--exact",
            "--nocapture",
        ],
        cwd=ROOT,
        env=env,
    ).returncode


if __name__ == "__main__":
    sys.exit(main())