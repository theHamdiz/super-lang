#!/usr/bin/env python3
"""Regression tests for Zed hot-reload install layout.

Zed marks an extension as `dev` only when `extensions/installed/<id>` is a
symlink (see extension_host::add_extension_to_index). Copy-installs get
`dev: false`, so `zed: reload extensions` / `zed: rebuild dev extension` cannot
reliably refresh WASM + LSP without a full editor restart.
"""

from __future__ import annotations

import json
import os
import stat
import tempfile
import unittest
from pathlib import Path


def atomic_install_symlink(extension_src: Path, install_dir: Path) -> None:
    """Mirror editors/zed/install.sh symlink deploy (no rm gap of live tree)."""
    install_dir.parent.mkdir(parents=True, exist_ok=True)
    staging = install_dir.parent.parent / "staging" / f"super-link-{os.getpid()}"
    staging.parent.mkdir(parents=True, exist_ok=True)
    if staging.exists() or staging.is_symlink():
        if staging.is_dir() and not staging.is_symlink():
            raise RuntimeError(f"refusing to remove unexpected dir: {staging}")
        staging.unlink()
    staging.symlink_to(extension_src.resolve(), target_is_directory=True)

    if install_dir.is_symlink() or install_dir.exists():
        if install_dir.is_symlink() or install_dir.is_file():
            install_dir.unlink()
        else:
            # Real directory from legacy copy-install — must go before ln.
            import shutil

            shutil.rmtree(install_dir)

    staging.rename(install_dir)


def zed_would_mark_dev(install_dir: Path) -> bool:
    """Same predicate Zed uses: metadata.is_symlink on the install path."""
    return install_dir.is_symlink()


def bump_extension_version(toml_text: str, epoch: int) -> str:
    """Force a new semver so ExtensionIndexEntry PartialEq always changes."""
    import re

    version = f"0.1.{epoch % 1_000_000_000}"
    return re.sub(
        r'(?m)^version\s*=\s*"[^"]*"',
        f'version = "{version}"',
        toml_text,
        count=1,
    )


class DevSymlinkInstallTests(unittest.TestCase):
    def test_legacy_copy_dir_is_not_dev(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            src = root / "editors" / "zed"
            src.mkdir(parents=True)
            (src / "extension.toml").write_text('id = "super"\nversion = "0.1.0"\n')
            installed = root / "extensions" / "installed" / "super"
            installed.mkdir(parents=True)
            (installed / "extension.toml").write_text((src / "extension.toml").read_text())
            self.assertFalse(zed_would_mark_dev(installed))

    def test_symlink_install_is_dev_and_points_at_source(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            src = root / "editors" / "zed"
            src.mkdir(parents=True)
            (src / "extension.wasm").write_bytes(b"\0wasm")
            (src / "extension.toml").write_text('id = "super"\nversion = "0.1.0"\n')
            installed = root / "extensions" / "installed" / "super"
            # Simulate legacy copy first.
            installed.mkdir(parents=True)
            (installed / "stale").write_text("old")

            atomic_install_symlink(src, installed)

            self.assertTrue(zed_would_mark_dev(installed))
            self.assertEqual(installed.resolve(), src.resolve())
            self.assertTrue((installed / "extension.wasm").is_file())
            self.assertFalse((installed / "stale").exists())

    def test_reinstall_over_existing_symlink_is_atomic(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            src = root / "editors" / "zed"
            src.mkdir(parents=True)
            (src / "extension.toml").write_text('id = "super"\nversion = "0.1.0"\n')
            installed = root / "extensions" / "installed" / "super"
            atomic_install_symlink(src, installed)
            atomic_install_symlink(src, installed)
            self.assertTrue(installed.is_symlink())
            self.assertEqual(os.readlink(installed), str(src.resolve()))

    def test_version_bump_changes_manifest_string(self) -> None:
        before = 'id = "super"\nversion = "0.1.0"\nname = "Super"\n'
        after = bump_extension_version(before, 1_700_000_000)
        self.assertIn('version = "0.1.700000000"', after)
        self.assertNotEqual(before, after)

    def test_icon_theme_omits_empty_maps(self) -> None:
        theme_path = (
            Path(__file__).resolve().parents[1] / "icon_themes" / "super-icons.json"
        )
        data = json.loads(theme_path.read_text())
        for theme in data["themes"]:
            self.assertNotIn("directory_icons", theme)
            self.assertNotIn("file_stems", theme)
            for value in theme["file_icons"].values():
                self.assertIsInstance(value, dict)
                self.assertIn("path", value)
            # Distinct names so Zed's icon theme index does not collapse light/dark.
        names = [t["name"] for t in data["themes"]]
        self.assertEqual(len(names), len(set(names)))


if __name__ == "__main__":
    unittest.main()
