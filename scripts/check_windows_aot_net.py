#!/usr/bin/env python3
"""Fail when docs contradict Windows AOT socket support (Round 7 Option A)."""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

EXACT = "Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0."

# Round 6 Option B wording must not linger.
FORBIDDEN_LEGACY = [
    "unsupported in 0.1.0 (Rust host WSAPoll only)",
    "Windows C23 AOT socket support is not claimed",
    "advanced Windows C-host networking is not claimed",
    "do not assume a C23 AOT",
]

FROZEN = {
    ROOT / "docs" / "REPORT.md",
    ROOT / "docs" / "results.json",
}

SCAN_SUFFIXES = {".md", ".html", ".txt", ".mdx"}
SCAN_ROOTS = [
    ROOT / "README.md",
    ROOT / "CHANGELOG.md",
    ROOT / "SPECIFICATION.md",
    ROOT / "SPECIFICATION.mdx",
    ROOT / "docs",
]


def carries_exact(text: str) -> bool:
    return EXACT in text or EXACT in text.replace("**", "")


def iter_files() -> list[Path]:
    out: list[Path] = []
    for base in SCAN_ROOTS:
        if base.is_file():
            if base not in FROZEN:
                out.append(base)
            continue
        if not base.is_dir():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in SCAN_SUFFIXES:
                continue
            if path in FROZEN:
                continue
            if "benchmarks/data" in path.as_posix():
                continue
            out.append(path)
    return sorted(set(out))


def check_platform_md(path: Path, errors: list[str]) -> None:
    text = path.read_text(encoding="utf-8")
    if not carries_exact(text):
        errors.append(f"{path}: missing exact Windows AOT sockets phrase")
    if "Windows x86_64" not in text:
        errors.append(f"{path}: missing Windows platform row")
    win_row = re.search(r"<tr><td>Windows x86_64</td>.*?</tr>", text, re.DOTALL)
    if win_row and "Winsock2" not in win_row.group(0):
        errors.append(f"{path}: Windows row AOT sockets column must mention Winsock2")


def main() -> int:
    errors: list[str] = []
    platform = ROOT / "docs" / "platform.md"
    if not platform.is_file():
        errors.append("docs/platform.md missing")
    else:
        check_platform_md(platform, errors)

    must_carry_exact = {
        ROOT / "README.md",
        ROOT / "CHANGELOG.md",
        ROOT / "docs" / "http-server.md",
        ROOT / "SPECIFICATION.md",
    }
    for path in must_carry_exact:
        if path.is_file() and not carries_exact(path.read_text(encoding="utf-8")):
            errors.append(f"{path}: must include exact phrase: {EXACT!r}")

    for path in iter_files():
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        rel = path.relative_to(ROOT)
        for legacy in FORBIDDEN_LEGACY:
            if legacy in text:
                errors.append(f"{rel}: stale Option-B wording {legacy!r}")

    # Round 9: AOT host lives in Rust (`aot_host/net_host_c.rs`), not `net_host.c`.
    net_path = (
        ROOT / "crates" / "super-runtime" / "src" / "aot_host" / "net_host_c.rs"
    )
    net = net_path.read_text(encoding="utf-8", errors="replace")
    if "WSAPoll" not in net or "WSAStartup" not in net:
        errors.append(
            "aot_host/net_host_c.rs: missing Winsock2 WSAStartup/WSAPoll implementation"
        )
    if "unsupported by Windows C host" in net:
        errors.append("aot_host/net_host_c.rs: still has the Option-B stub return")
    if (ROOT / "crates" / "super-runtime" / "src" / "net_host.c").exists():
        errors.append("net_host.c must not remain; AOT host is aot_host/net_host_c.rs")

    if errors:
        print("check_windows_aot_net: FAILED", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        return 1

    print(f"check_windows_aot_net: OK ({EXACT})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
