#!/usr/bin/env python3
"""LAW-BENCH-001 / Gate B: benchmarks must not import or call `host`."""
from __future__ import annotations

import unittest
from pathlib import Path
from unittest import mock

import runner

ROOT = Path(__file__).resolve().parent
FIXTURES = ROOT / "tests" / "fixtures"


class HostBanCheckerTests(unittest.TestCase):
    def test_clean_fixture_passes(self) -> None:
        src = (FIXTURES / "clean_algo.sr").read_text(encoding="utf-8")
        self.assertIsNone(runner.check_algo_bans_host(src))

    def test_bad_host_shim_fails(self) -> None:
        src = (FIXTURES / "bad_host_shim.sr").read_text(encoding="utf-8")
        err = runner.check_algo_bans_host(src)
        self.assertIsNotNone(err)
        assert err is not None
        self.assertIn(runner.LAW_BENCH_001, err)
        self.assertIn("E0504", err)

    def test_host_dot_alone_fails(self) -> None:
        err = runner.check_algo_bans_host('cs = host.sys_sparse12_run()\n')
        self.assertIsNotNone(err)

    def test_comment_host_mention_ok(self) -> None:
        src = "// was host.sys_x_run\nuse std.math.core\n"
        self.assertIsNone(runner.check_algo_bans_host(src))

    def test_braced_use_host_fails(self) -> None:
        self.assertIsNotNone(runner.check_algo_bans_host("use { host, math }\n"))


class VerifySuperGateTests(unittest.TestCase):
    def test_verify_super_rejects_host_before_build(self) -> None:
        import tempfile

        tools = {
            "super": runner.Toolchain(
                name="super", path="/usr/bin/true", version="test", available=True
            )
        }
        with tempfile.TemporaryDirectory(prefix="bench_host_gate_") as td:
            algo = Path(td)
            (algo / "algo.sr").write_text(
                (FIXTURES / "bad_host_shim.sr").read_text(encoding="utf-8"),
                encoding="utf-8",
            )
            with mock.patch.object(runner, "run_cmd") as run_cmd:
                ok, cs, detail = runner.verify_super(tools, algo)
                run_cmd.assert_not_called()
            self.assertFalse(ok)
            self.assertIsNone(cs)
            self.assertIn(runner.LAW_BENCH_001, detail)

    def test_verify_super_allows_clean_past_host_gate(self) -> None:
        """Host gate passes; build may still fail without a real super binary."""
        import tempfile

        tools = {
            "super": runner.Toolchain(
                name="super", path="/nonexistent/super", version="x", available=True
            )
        }
        with tempfile.TemporaryDirectory(prefix="bench_clean_gate_") as td:
            algo = Path(td)
            (algo / "algo.sr").write_text(
                (FIXTURES / "clean_algo.sr").read_text(encoding="utf-8"),
                encoding="utf-8",
            )
            ok, _cs, detail = runner.verify_super(tools, algo)
            # Build fails because binary missing/broken — but NOT with LAW-BENCH-001.
            self.assertFalse(ok)
            self.assertNotIn(runner.LAW_BENCH_001, detail)


class SuiteAlgoSourcesTests(unittest.TestCase):
    def test_all_algo_sr_ban_host(self) -> None:
        algos = sorted(ROOT.glob("*/algo.sr"))
        self.assertGreaterEqual(len(algos), 20)
        for path in algos:
            with self.subTest(algo=path.parent.name):
                err = runner.check_algo_bans_host(path.read_text(encoding="utf-8"))
                self.assertIsNone(err, msg=f"{path}: {err}")


if __name__ == "__main__":
    unittest.main()
