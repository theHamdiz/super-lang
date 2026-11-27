#!/usr/bin/env python3
"""
Fuzz the std.html.template Rust parser.
Duration: SUPER_TEMPLATE_FUZZ_SECS (default 600 for CI, 30 locally if unset→use 30).

Invokes `cargo test -p super-frontend template_fuzz --lib` which reads the env var.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    secs = os.environ.get("SUPER_TEMPLATE_FUZZ_SECS", "30")
    env = os.environ.copy()
    env["SUPER_TEMPLATE_FUZZ_SECS"] = secs
    r = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "super-frontend",
            "template::tests::fuzz_timed",
            "--lib",
            "--",
            "--exact",
            "--nocapture",
        ],
        cwd=ROOT,
        env=env,
    )
    return r.returncode


if __name__ == "__main__":
    sys.exit(main())
