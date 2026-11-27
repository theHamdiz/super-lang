#!/usr/bin/env python3
"""Unit tests for composite scoring (weights, cycle, reliability)."""
from __future__ import annotations

import math
import unittest

import runner


def _algo(name: str, targets: list[dict]) -> dict:
    return {"name": name, "gate": "ok", "reference_checksum": "1", "targets": targets}


def _ok(target: str, mean: float, rss: int, *, cycle: float | None = None, median: float | None = None) -> dict:
    med = median if median is not None else mean
    return {
        "target": target,
        "status": "ok",
        "stats": {"mean": mean, "median": med, "primary": med, "n": 9.0},
        "peak_rss_kb": rss,
        "cycle_s": cycle if cycle is not None else mean,
        "checksum": "1",
    }


def _fail(target: str) -> dict:
    return {"target": target, "status": "failed", "stats": {}, "detail": "nope"}


class CycleCostTests(unittest.TestCase):
    def test_interpreter_ignores_compile(self) -> None:
        self.assertEqual(runner.cycle_cost_s("python", 12.0, 0.5), 0.5)
        self.assertEqual(runner.cycle_cost_s("super_jit", 9.0, 1.0), 1.0)

    def test_aot_adds_compile(self) -> None:
        self.assertEqual(runner.cycle_cost_s("super_aot", 2.5, 0.5), 3.0)
        self.assertEqual(runner.cycle_cost_s("rust", 1.0, 0.2), 1.2)

    def test_missing_run(self) -> None:
        self.assertIsNone(runner.cycle_cost_s("c", 1.0, None))


class CompositeScoreTests(unittest.TestCase):
    def test_weights_sum_to_one(self) -> None:
        self.assertAlmostEqual(sum(runner.SCORE_WEIGHTS.values()), 1.0)
        self.assertNotIn("cpu", runner.SCORE_WEIGHTS)
        self.assertEqual(runner.SCORE_WEIGHTS["loc"], 0.10)
        self.assertEqual(runner.SCORE_WEIGHTS["cycle"], 0.10)
        self.assertEqual(runner.SCORE_WEIGHTS["reliability"], 0.15)

    def test_cpu_removed_and_reliability_ranks_pass_rate(self) -> None:
        report = {
            "loc": {
                "by_language": {
                    "c": {"code": 100},
                    "super": {"code": 80},
                    "rust": {"code": 120},
                    "bun": {"code": 90},
                    "kotlin": {"code": 110},
                    "lua": {"code": 95},
                    "python": {"code": 100},
                    "julia": {"code": 100},
                    "zig": {"code": 100},
                }
            },
            "algorithms": [
                _algo(
                    "01_nbody",
                    [
                        _ok("c", 1.0, 3000, cycle=1.2),
                        _ok("super_aot", 1.1, 2800, cycle=3.0),
                        _ok("rust", 0.9, 3100, cycle=2.0),
                        _fail("bun"),
                        _ok("kotlin", 2.0, 5000, cycle=5.0),
                        _ok("lua", 1.5, 3300, cycle=1.5),
                        _ok("python", 3.0, 8000, cycle=3.0),
                        _ok("julia", 1.2, 4000, cycle=1.2),
                        _ok("zig", 1.0, 2500, cycle=1.8),
                        _ok("super_jit", 2.0, 6000, cycle=2.0),
                    ],
                ),
                _algo(
                    "02_mandelbrot_parallel",
                    [
                        _ok("c", 1.0, 3000, cycle=1.2),
                        _ok("super_aot", 1.0, 2700, cycle=2.8),
                        _fail("rust"),
                        _fail("bun"),
                        _ok("kotlin", 2.0, 5000, cycle=5.0),
                        _ok("lua", 1.5, 3300, cycle=1.5),
                        _ok("python", 3.0, 8000, cycle=3.0),
                        _ok("julia", 1.2, 4000, cycle=1.2),
                        _ok("zig", 1.0, 2500, cycle=1.8),
                        _ok("super_jit", 2.0, 6000, cycle=2.0),
                    ],
                ),
            ],
        }
        runner.attach_composite_scores(report)
        comp = report["composite_scores"]
        self.assertNotIn("cpu", comp["c"])
        self.assertIn("reliability", comp["c"])
        self.assertIn("cycle", comp["c"])
        self.assertIn("contrib", comp["c"])
        # rust failed one of two → lower reliability than perfect-pass languages
        self.assertLess(comp["rust"]["reliability"], comp["c"]["reliability"])
        self.assertLess(comp["bun"]["reliability"], comp["c"]["reliability"])
        # contributions sum to score
        for t in runner.TARGETS:
            parts = comp[t]["contrib"]
            self.assertAlmostEqual(sum(parts.values()), comp[t]["score"], places=2)
        # weights documented
        self.assertEqual(report["score_weights"]["speed"], 0.40)
        self.assertIn("removed", report["methodology"])

    def test_speed_best_gets_100(self) -> None:
        report = {
            "loc": {"by_language": {k: {"code": 100} for k in (
                "c", "super", "bun", "kotlin", "lua", "python", "julia", "rust", "zig"
            )}},
            "algorithms": [
                _algo(
                    "01_nbody",
                    [
                        _ok(t, 1.0 if t != "zig" else 0.5, 3000, cycle=1.0)
                        for t in runner.TARGETS
                    ],
                )
            ],
        }
        runner.attach_composite_scores(report)
        self.assertEqual(report["composite_scores"]["zig"]["speed"], 100.0)
        self.assertLess(report["composite_scores"]["c"]["speed"], 100.0)


class GeoMeanTests(unittest.TestCase):
    def test_geo_mean(self) -> None:
        self.assertAlmostEqual(runner.geo_mean([1.0, 4.0]), 2.0)
        self.assertIsNone(runner.geo_mean([]))
        self.assertIsNone(runner.geo_mean([1.0, 0.0]))


class StatsAggregatorTests(unittest.TestCase):
    """Timed-run aggregator: median primary, warmups never passed in."""

    def test_empty(self) -> None:
        st = runner.stats([])
        self.assertEqual(st["n"], 0.0)
        self.assertEqual(st["primary"], 0.0)
        self.assertEqual(st["median"], 0.0)
        self.assertEqual(st["mean"], 0.0)

    def test_odd_list_median_is_middle_sample(self) -> None:
        # 9 timed samples (warmup already discarded by caller).
        samples = [9.0, 1.0, 5.0, 3.0, 7.0, 2.0, 8.0, 4.0, 6.0]
        st = runner.stats(samples)
        self.assertEqual(st["n"], 9.0)
        self.assertEqual(st["min"], 1.0)
        self.assertEqual(st["max"], 9.0)
        self.assertEqual(st["median"], 5.0)
        self.assertEqual(st["primary"], 5.0)
        self.assertAlmostEqual(st["mean"], 5.0)

    def test_even_list_median_averages_pair(self) -> None:
        samples = [10.0, 2.0, 8.0, 4.0]  # sorted 2,4,8,10 → median 6
        st = runner.stats(samples)
        self.assertEqual(st["n"], 4.0)
        self.assertEqual(st["median"], 6.0)
        self.assertEqual(st["primary"], 6.0)
        self.assertAlmostEqual(st["mean"], 6.0)

    def test_outlier_skews_mean_not_median(self) -> None:
        samples = [1.0, 1.1, 1.0, 1.05, 1.02, 1.08, 1.01, 1.03, 50.0]
        st = runner.stats(samples)
        self.assertLess(st["median"], 1.2)
        self.assertGreater(st["mean"], 5.0)
        self.assertEqual(st["primary"], st["median"])

    def test_wall_primary_prefers_primary_then_median(self) -> None:
        self.assertEqual(
            runner.wall_primary({"primary": 1.5, "median": 2.0, "mean": 3.0}),
            1.5,
        )
        self.assertEqual(
            runner.wall_primary({"median": 2.0, "mean": 3.0}),
            2.0,
        )
        self.assertEqual(runner.wall_primary({"mean": 3.0}), 3.0)
        self.assertIsNone(runner.wall_primary({}))
        self.assertIsNone(runner.wall_primary(None))

    def test_single_sample(self) -> None:
        st = runner.stats([0.042])
        self.assertEqual(st["median"], 0.042)
        self.assertEqual(st["mean"], 0.042)
        self.assertEqual(st["primary"], 0.042)
        self.assertEqual(st["stddev"], 0.0)

    def test_policy_defaults(self) -> None:
        self.assertGreaterEqual(runner.TIMED_RUNS, 7)
        self.assertGreaterEqual(runner.WARMUPS, 1)
        self.assertLessEqual(runner.WARMUPS, 2)
        self.assertEqual(runner.WALL_PRIMARY, "median")
        self.assertEqual(runner.ESTIMATOR, "median")
        self.assertRegex(runner.PROTOCOL_VERSION, r"^\d+\.\d+\.\d+$")

    def test_composite_uses_median_not_mean_when_they_differ(self) -> None:
        # zig: median fast, mean dragged by one outlier field we don't use for score
        report = {
            "loc": {
                "by_language": {
                    k: {"code": 100}
                    for k in (
                        "c",
                        "super",
                        "bun",
                        "kotlin",
                        "lua",
                        "python",
                        "julia",
                        "rust",
                        "zig",
                    )
                }
            },
            "algorithms": [
                _algo(
                    "01_nbody",
                    [
                        _ok("zig", mean=10.0, rss=3000, median=0.5, cycle=1.0),
                        *[
                            _ok(t, 1.0, 3000, cycle=1.0)
                            for t in runner.TARGETS
                            if t != "zig"
                        ],
                    ],
                )
            ],
        }
        runner.attach_composite_scores(report)
        self.assertEqual(report["composite_scores"]["zig"]["speed"], 100.0)
        self.assertEqual(report["methodology"]["wall_primary"], "median")
        self.assertEqual(report["methodology"]["protocol_version"], runner.PROTOCOL_VERSION)
        self.assertIn("timed_runs", report["methodology"])
        self.assertIn("within_noise", report["methodology"])


class ParseOnlyTests(unittest.TestCase):
    def test_only_flag(self) -> None:
        self.assertEqual(
            runner.parse_only_algos(["--only", "13_polars_data_pipeline"]),
            ["13_polars_data_pipeline"],
        )
        self.assertEqual(runner.parse_only_algos(["--only", "13"]), ["13_polars_data_pipeline"])
        self.assertIsNone(runner.parse_only_algos([]))


if __name__ == "__main__":
    unittest.main()

