#!/usr/bin/env python3
"""Acceptance tests for timing protocol v2: warm-up, interleave, median, overlap, version."""
from __future__ import annotations

import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Callable
from unittest import mock

import runner


def _mk_run(times: list[float], checksum: str = "CS") -> Callable[[], tuple[int, str, str, float]]:
    """Return a runner that yields successive wall times with a fixed checksum."""
    it = iter(times)

    def run() -> tuple[int, str, str, float]:
        try:
            dt = next(it)
        except StopIteration as e:
            raise AssertionError("ran out of injected samples") from e
        return 0, f"CHECKSUM={checksum}\n", "", float(dt)

    return run


class ProtocolDefaultsTests(unittest.TestCase):
    def test_protocol_version_is_frozen_string(self) -> None:
        self.assertIsInstance(runner.PROTOCOL_VERSION, str)
        self.assertRegex(runner.PROTOCOL_VERSION, r"^\d+\.\d+\.\d+$")

    def test_defaults_sized_for_sub10ms(self) -> None:
        self.assertGreaterEqual(runner.TIMED_RUNS, 7)
        self.assertGreaterEqual(runner.WARMUPS, 1)
        self.assertLessEqual(runner.WARMUPS, 2)
        self.assertIn(runner.ESTIMATOR, ("median", "trimmed_mean", "mean"))
        self.assertEqual(runner.WALL_PRIMARY, runner.ESTIMATOR if runner.ESTIMATOR != "mean" else "median")

    def test_protocol_block_embeds_version(self) -> None:
        p = runner.protocol_block()
        self.assertEqual(p["version"], runner.PROTOCOL_VERSION)
        self.assertEqual(p["warmups"], runner.WARMUPS)
        self.assertEqual(p["timed_runs"], runner.TIMED_RUNS)
        self.assertEqual(p["estimator"], runner.ESTIMATOR)
        self.assertIn("round_robin", p["interleave"])


class AggregatorTests(unittest.TestCase):
    def test_median_odd_and_even(self) -> None:
        odd = runner.stats([9, 1, 5, 3, 7, 2, 8, 4, 6], estimator="median")
        self.assertEqual(odd["median"], 5.0)
        self.assertEqual(odd["primary"], 5.0)
        self.assertEqual(odd["estimator"], "median")
        even = runner.stats([10, 2, 8, 4], estimator="median")
        self.assertEqual(even["median"], 6.0)
        self.assertEqual(even["primary"], 6.0)

    def test_warmup_excluded_via_raw_runs(self) -> None:
        raw = [
            {"time_s": 99.0, "warmup": True, "phase": "warmup", "index": 0},
            {"time_s": 98.0, "warmup": True, "phase": "warmup", "index": 1},
            {"time_s": 1.0, "warmup": False, "phase": "timed", "index": 0},
            {"time_s": 1.1, "warmup": False, "phase": "timed", "index": 1},
            {"time_s": 1.05, "warmup": False, "phase": "timed", "index": 2},
        ]
        timed = runner.timed_samples_from_raw(raw)
        self.assertEqual(timed, [1.0, 1.1, 1.05])
        st = runner.reaggregate_raw_runs(raw, estimator="median")
        self.assertEqual(st["n"], 3.0)
        self.assertEqual(st["median"], 1.05)
        self.assertEqual(st["primary"], 1.05)
        self.assertLess(st["max"], 2.0)

    def test_trimmed_mean_drops_tails(self) -> None:
        # 10 samples → drop 10% each side = 1 each → mean of middle 8
        samples = [100.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]
        st = runner.stats(samples, estimator="trimmed_mean")
        self.assertEqual(st["estimator"], "trimmed_mean")
        self.assertLess(st["primary"], 2.0)
        self.assertGreater(st["mean"], 10.0)

    def test_spread_fields_present(self) -> None:
        st = runner.stats([1.0, 1.1, 0.9, 1.05, 0.95])
        for key in ("stddev", "stderr", "ci_lo", "ci_hi", "min", "max"):
            self.assertIn(key, st)
        self.assertLessEqual(st["ci_lo"], st["mean"])
        self.assertGreaterEqual(st["ci_hi"], st["mean"])
        self.assertGreater(st["stddev"], 0.0)


class InterleaveOrderTests(unittest.TestCase):
    def test_round_robin_alternates_languages(self) -> None:
        sched = runner.interleave_schedule(
            ["rust", "zig", "super_aot"],
            warmups=2,
            timed=3,
        )
        # Warm phase: rust,zig,super × 2
        self.assertEqual(
            [(t, p) for t, p, _ in sched[:6]],
            [
                ("rust", "warmup"),
                ("zig", "warmup"),
                ("super_aot", "warmup"),
                ("rust", "warmup"),
                ("zig", "warmup"),
                ("super_aot", "warmup"),
            ],
        )
        # Timed phase interleaved, not blocked per language
        timed = [(t, p, i) for t, p, i in sched if p == "timed"]
        self.assertEqual(len(timed), 9)
        self.assertEqual(
            [t for t, _, _ in timed[:3]],
            ["rust", "zig", "super_aot"],
        )
        self.assertEqual(
            [t for t, _, _ in timed[3:6]],
            ["rust", "zig", "super_aot"],
        )

    def test_extra_warm_only_extends_that_language(self) -> None:
        sched = runner.interleave_schedule(
            ["c", "kotlin"],
            warmups=2,
            timed=1,
            extra_warm={"kotlin": 1},
        )
        warms = [(t, i) for t, p, i in sched if p == "warmup"]
        self.assertEqual(warms.count(("c", 0)) + warms.count(("c", 1)), 2)
        self.assertEqual(len([1 for t, _ in warms if t == "kotlin"]), 3)


class OverlapAndSyntheticSuiteTests(unittest.TestCase):
    def test_overlapping_ci_flagged_within_noise(self) -> None:
        # Tight clusters that overlap
        a = runner.stats([10.0, 10.1, 9.9, 10.05, 9.95, 10.02, 9.98, 10.0, 10.03, 9.97])
        b = runner.stats([10.05, 10.15, 9.95, 10.1, 10.0, 10.08, 9.92, 10.04, 10.06, 9.99])
        self.assertTrue(runner.intervals_overlap(a, b))
        ranks = runner.ranking_within_noise({"fast": a, "slow": b})
        by = {r["target"]: r for r in ranks}
        self.assertIn("slow", by["fast"]["within_noise_of"])
        self.assertFalse(by["fast"]["hard_order"])

    def test_separated_ci_is_hard_order(self) -> None:
        fast = runner.stats([1.0] * 10)
        slow = runner.stats([5.0] * 10)
        self.assertFalse(runner.intervals_overlap(fast, slow))
        ranks = runner.ranking_within_noise({"fast": fast, "slow": slow})
        self.assertEqual(ranks[0]["target"], "fast")
        self.assertTrue(ranks[0]["hard_order"])
        self.assertEqual(ranks[0]["within_noise_of"], [])

    def test_synthetic_interleaved_harness_reports_spread_and_noise(self) -> None:
        """Injected noisy walls: overlapping langs flagged; raw warmups marked."""
        # 2 warm + 10 timed each. Overlapping A/B; C clearly slower.
        warm = [50.0, 40.0]
        # A/B share the same noisy band (within noise); C is far slower.
        a_timed = [10.0, 10.2, 9.8, 10.1, 9.9, 10.05, 9.95, 10.15, 9.85, 10.0]
        b_timed = [10.05, 10.25, 9.85, 10.15, 9.95, 10.1, 10.0, 10.2, 9.9, 10.05]
        c_timed = [30.0, 30.2, 29.8, 30.1, 29.9, 30.05, 29.95, 30.15, 29.85, 30.0]
        runners = {
            "rust": _mk_run(warm + a_timed),
            "zig": _mk_run(warm + b_timed),
            "c": _mk_run(warm + c_timed),
        }
        with mock.patch.object(runner, "WARMUPS", 2), mock.patch.object(runner, "TIMED_RUNS", 10):
            with mock.patch.object(runner, "TARGETS", ["rust", "zig", "c"]):
                results = runner.time_targets_round_robin(runners, "CS")
        for t in ("rust", "zig", "c"):
            self.assertEqual(results[t].status, "ok")
            raw = results[t].raw_runs
            self.assertEqual(sum(1 for r in raw if r["warmup"]), 2)
            self.assertEqual(sum(1 for r in raw if not r["warmup"]), 10)
            self.assertEqual(len(results[t].times_s), 10)
            st = results[t].stats
            self.assertGreater(st["stddev"], 0.0)
            self.assertIn("ci_lo", st)
            self.assertEqual(st["estimator"], "median")
        # A/B overlap ⇒ within noise; C separated
        self.assertIn("within noise", results["rust"].ranking_note)
        self.assertIn("zig", results["rust"].ranking_note)
        self.assertTrue(
            results["c"].ranking_note.startswith("hard order")
            or "within noise" not in results["c"].ranking_note
            or "rust" not in (results["c"].ranking_note)
        )


class ProtocolBumpAndReaggregateTests(unittest.TestCase):
    def test_changing_only_protocol_bumps_recorded_version(self) -> None:
        report = {
            "loc": {"by_language": {k: {"code": 100} for k in (
                "c", "super", "bun", "kotlin", "lua", "python", "julia", "rust", "zig"
            )}},
            "algorithms": [],
            "protocol": {"version": "1.0.0"},
        }
        with mock.patch.object(runner, "PROTOCOL_VERSION", "9.9.9"):
            runner.attach_composite_scores(report)
        self.assertEqual(report["methodology"]["protocol_version"], "9.9.9")
        self.assertEqual(report["protocol"]["version"], "9.9.9")

    def test_reaggregate_from_raw_reproduces_median(self) -> None:
        raw = [
            {"time_s": 9.0, "warmup": True},
            {"time_s": 1.0, "warmup": False},
            {"time_s": 3.0, "warmup": False},
            {"time_s": 2.0, "warmup": False},
        ]
        report = {
            "loc": {"by_language": {k: {"code": 10} for k in (
                "c", "super", "bun", "kotlin", "lua", "python", "julia", "rust", "zig"
            )}},
            "algorithms": [
                {
                    "name": "01_nbody",
                    "gate": "ok",
                    "targets": [
                        {
                            "target": t,
                            "status": "ok",
                            "raw_runs": list(raw),
                            "times_s": [1.0, 3.0, 2.0],
                            "stats": {"mean": 99.0, "median": 99.0, "primary": 99.0, "n": 3},
                            "peak_rss_kb": 1000,
                            "cycle_s": 1.0,
                            "checksum": "1",
                        }
                        for t in runner.TARGETS
                    ],
                }
            ],
            "protocol": {"version": "1.0.0"},
        }
        out = runner.reaggregate_report(report, estimator="median")
        st = out["algorithms"][0]["targets"][0]["stats"]
        self.assertEqual(st["median"], 2.0)
        self.assertEqual(st["primary"], 2.0)
        self.assertEqual(st["n"], 3.0)
        self.assertTrue(out["protocol"].get("reaggregated"))
        self.assertEqual(out["protocol"]["version"], runner.PROTOCOL_VERSION)

    def test_cli_config_overrides_globals(self) -> None:
        old = (runner.WARMUPS, runner.TIMED_RUNS, runner.ESTIMATOR, runner.WALL_PRIMARY)
        try:
            rest = runner.apply_cli_config(
                ["--warm", "1", "--timed=12", "--estimator", "trimmed_mean", "--only", "13"]
            )
            self.assertEqual(runner.WARMUPS, 1)
            self.assertEqual(runner.TIMED_RUNS, 12)
            self.assertEqual(runner.ESTIMATOR, "trimmed_mean")
            self.assertEqual(runner.WALL_PRIMARY, "trimmed_mean")
            self.assertEqual(rest, ["--only", "13"])
        finally:
            runner.WARMUPS, runner.TIMED_RUNS, runner.ESTIMATOR, runner.WALL_PRIMARY = old


class RoundRobinExecutionOrderTests(unittest.TestCase):
    def test_execution_order_is_not_language_blocked(self) -> None:
        order: list[str] = []

        def make(name: str) -> Callable[[], tuple[int, str, str, float]]:
            n = {"i": 0}

            def run() -> tuple[int, str, str, float]:
                order.append(name)
                n["i"] += 1
                return 0, "CHECKSUM=1\n", "", 0.01

            return run

        runners = {t: make(t) for t in ("rust", "zig", "c")}
        with mock.patch.object(runner, "WARMUPS", 1), mock.patch.object(runner, "TIMED_RUNS", 2):
            with mock.patch.object(runner, "TARGETS", ["rust", "zig", "c"]):
                runner.time_targets_round_robin(runners, "1")
        # First three calls are one warm each language, not three rusts
        self.assertEqual(order[:3], ["rust", "zig", "c"])
        self.assertEqual(order[3:6], ["rust", "zig", "c"])


if __name__ == "__main__":
    unittest.main()
