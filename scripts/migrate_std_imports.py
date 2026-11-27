#!/usr/bin/env python3
"""Rewrite short std imports to fully qualified manifest paths (idempotent)."""

from __future__ import annotations

import re
import sys
from pathlib import Path

# Longest / most specific keys first to avoid partial replacements.
SHORT_TO_MANIFEST: list[tuple[str, str]] = [
    ("std.io.stdio", "std.io.stdio"),
    ("std.io.socket", "std.io.socket"),
    ("std.io.uring", "std.io.uring"),
    ("std.io.epoll", "std.io.epoll"),
    ("std.io.poll", "std.io.poll"),
    ("std.io.buf", "std.io.buf"),
    ("std.io.fs", "std.io.fs"),
    ("std.sys.epoll", "std.io.epoll"),
    ("std.sys.io_uring", "std.io.uring"),
    ("std.net.poll", "std.net.poll"),
    ("std.net.http", "std.net.http"),
    ("std.net.ws", "std.net.ws"),
    ("std.net.url", "std.net.url"),
    ("std.net.tcp", "std.net.tcp"),
    ("std.net.udp", "std.net.udp"),
    ("std.net.dns", "std.net.dns"),
    ("std.net.ip", "std.net.ip"),
    ("std.net.sse", "std.net.sse"),
    ("std.net.retry", "std.net.retry"),
    ("std.net.udp_netcode", "std.net.udp_netcode"),
    ("std.db.migrate", "std.db.migrate"),
    ("std.db.postgres", "std.db.postgres"),
    ("std.db.redis", "std.db.redis"),
    ("std.db.sqlite", "std.db.sqlite"),
    ("std.db.sql", "std.db.sql"),
    ("std.db.pg", "std.db.postgres"),
    ("std.core.str", "std.core.str"),
    ("std.core.bytes", "std.core.bytes"),
    ("std.core.collections", "std.core.collections"),
    ("std.core.regex", "std.core.regex"),
    ("std.core.uuid", "std.core.uuid"),
    ("std.fmt.json", "std.fmt.json"),
    ("std.fmt.bin", "std.fmt.bin"),
    ("std.fmt.toml", "std.fmt.toml"),
    ("std.fmt.csv", "std.fmt.csv"),
    ("std.fmt.compress", "std.fmt.compress"),
    ("std.fmt.mime", "std.fmt.mime"),
    ("std.sys.fs", "std.sys.fs"),
    ("std.sys.path", "std.sys.path"),
    ("std.sys.cli", "std.sys.cli"),
    ("std.sys.env", "std.sys.env"),
    ("std.sys.proc", "std.sys.proc"),
    ("std.sys.time", "std.sys.time"),
    ("std.sys.window", "std.sys.window"),
    ("std.sys.sync", "std.sys.sync"),
    ("std.sec.crypto", "std.sec.crypto"),
    ("std.sec.encoding", "std.sec.encoding"),
    ("std.sec.jwt", "std.sec.jwt"),
    ("std.sec.sha2", "std.sec.sha2"),
    ("std.sec.hash", "std.sec.hash"),
    ("std.sec.pq", "std.sec.pq"),
    ("std.math.core", "std.math.core"),
    ("std.math.linalg", "std.math.linalg"),
    ("std.math.rand", "std.math.rand"),
    ("std.math.noise", "std.math.noise"),
    ("std.math.complex", "std.math.complex"),
    ("std.math.quat", "std.math.quat"),
    ("std.math.stats", "std.math.stats"),
    ("std.math.geom", "std.math.geom"),
    ("std.game.ecs", "std.game.ecs"),
    ("std.game.physics2d", "std.game.physics2d"),
    ("std.game.ai", "std.game.ai"),
    ("std.game.app", "std.game.app"),
    ("std.game.assets", "std.game.assets"),
    ("std.game.tilemap", "std.game.tilemap"),
    ("std.diag.test", "std.diag.test"),
    ("std.diag.log", "std.diag.log"),
    ("std.diag.metrics", "std.diag.metrics"),
    # Short legacy aliases → manifest
    ("std.io_uring", "std.io.uring"),
    ("std.epoll", "std.io.epoll"),
    ("std.collections", "std.core.collections"),
    ("std.bytes", "std.core.bytes"),
    ("std.regex", "std.core.regex"),
    ("std.uuid", "std.core.uuid"),
    ("std.str", "std.core.str"),
    ("std.json", "std.fmt.json"),
    ("std.bin", "std.fmt.bin"),
    ("std.toml", "std.fmt.toml"),
    ("std.csv", "std.fmt.csv"),
    ("std.compress", "std.fmt.compress"),
    ("std.mime", "std.fmt.mime"),
    ("std.fs", "std.sys.fs"),
    ("std.path", "std.sys.path"),
    ("std.cli", "std.sys.cli"),
    ("std.env", "std.sys.env"),
    ("std.proc", "std.sys.proc"),
    ("std.time", "std.sys.time"),
    ("std.window", "std.sys.window"),
    ("std.sync", "std.sys.sync"),
    ("std.http", "std.net.http"),
    ("std.ws", "std.net.ws"),
    ("std.url", "std.net.url"),
    ("std.ip", "std.net.ip"),
    ("std.tcp", "std.net.tcp"),
    ("std.udp", "std.net.udp"),
    ("std.dns", "std.net.dns"),
    ("std.sse", "std.net.sse"),
    ("std.retry", "std.net.retry"),
    ("std.poll", "std.net.poll"),
    ("std.netcode", "std.net.udp_netcode"),
    ("std.migrate", "std.db.migrate"),
    ("std.db", "std.db.sql"),
    ("std.crypto", "std.sec.crypto"),
    ("std.encoding", "std.sec.encoding"),
    ("std.jwt", "std.sec.jwt"),
    ("std.sha2", "std.sec.sha2"),
    ("std.hash", "std.sec.hash"),
    ("std.pq", "std.sec.pq"),
    ("std.math", "std.math.core"),
    ("std.linalg", "std.math.linalg"),
    ("std.simd", "std.math.linalg"),
    ("std.rand", "std.math.rand"),
    ("std.noise", "std.math.noise"),
    ("std.complex", "std.math.complex"),
    ("std.quat", "std.math.quat"),
    ("std.stats", "std.math.stats"),
    ("std.geom", "std.math.geom"),
    ("std.ecs", "std.game.ecs"),
    ("std.physics2d", "std.game.physics2d"),
    ("std.ai", "std.game.ai"),
    ("std.app", "std.game.app"),
    ("std.input", "std.game.app"),
    ("std.audio", "std.game.app"),
    ("std.assets", "std.game.assets"),
    ("std.tilemap", "std.game.tilemap"),
    ("std.test", "std.diag.test"),
    ("std.log", "std.diag.log"),
    ("std.metrics", "std.diag.metrics"),
]

USE_RE = re.compile(r"^(\s*use\s+)(std\.[^\s{]+)")


def migrate_line(line: str) -> str:
    m = USE_RE.match(line.rstrip("\r\n"))
    if not m:
        return line
    prefix, path = m.group(1), m.group(2)
    rest = line[m.end() :]
    for short, manifest in SHORT_TO_MANIFEST:
        if path == short:
            return f"{prefix}{manifest}{rest}"
    return line


def migrate_text(text: str) -> str:
    return "\n".join(migrate_line(ln) for ln in text.splitlines()) + (
        "" if not text.endswith("\n") else "\n"
    )


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    globs = ["stdlib/**/*.sr", "examples/**/*.sr", "benchmarks/**/*.sr", "tests/**/*.sr"]
    changed = 0
    for pattern in globs:
        for path in root.glob(pattern):
            old = path.read_text(encoding="utf-8")
            new = migrate_text(old)
            if new != old:
                path.write_text(new, encoding="utf-8")
                changed += 1
                print(path.relative_to(root))
    print(f"migrated {changed} file(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())