#!/usr/bin/env python3
"""Compile complete Super modules embedded in maintained Markdown sources."""
from __future__ import annotations

import argparse
import re
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FENCE = re.compile(r"```(?:super|sr)\s*\n(.*?)```", re.DOTALL)


def sources() -> list[Path]:
    return [
        ROOT / "BOOK.md",
        ROOT / "SPECIFICATION.md",
        *sorted((ROOT / "book" / "parts").glob("*.md")),
        *sorted((ROOT / "docs").glob("*.md")),
    ]


def complete_modules(path: Path) -> list[tuple[int, str]]:
    text = path.read_text(encoding="utf-8")
    modules: list[tuple[int, str]] = []
    for match in FENCE.finditer(text):
        code = match.group(1).strip() + "\n"
        line = text.count("\n", 0, match.start()) + 1
        # Test-only fragments depend on declarations in surrounding examples;
        # independently compilable blocks own an entry point.
        is_module = "+main(" in code
        is_diagnostic = "super check:" in code or "…" in code
        imports = re.findall(r"(?m)^\s*use\s+([A-Za-z_][A-Za-z0-9_.]*)", code)
        needs_sibling_file = any(not module.startswith("std.") for module in imports)
        if is_module and not is_diagnostic and not needs_sibling_file:
            modules.append((line, code))
    return modules


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--super", default=str(ROOT / "target" / "debug" / "super"))
    args = parser.parse_args()
    binary = Path(args.super)
    if not binary.is_file():
        parser.error(f"Super binary not found: {binary}")

    checked = 0
    failures: list[str] = []
    with tempfile.TemporaryDirectory(prefix="super-doc-tests-") as directory:
        temp = Path(directory)
        for source in sources():
            for index, (line, code) in enumerate(complete_modules(source)):
                module = temp / f"{source.stem}-{line}-{index}.sr"
                module.write_text(code, encoding="utf-8")
                result = subprocess.run(
                    [str(binary), "check", str(module)],
                    cwd=ROOT,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
                checked += 1
                if result.returncode:
                    failures.append(f"{source.relative_to(ROOT)}:{line}\n{result.stdout}")

    if failures:
        print("\n".join(failures))
        print(f"doc code: {len(failures)} failed / {checked} checked")
        return 1
    print(f"doc code: {checked} complete modules checked")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
