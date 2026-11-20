"""Focused standard-library tests for the S8-04 benchmark helpers (no guest run)."""

from __future__ import annotations

import unittest

import windows_boot_bench as bench


class SummarizeTests(unittest.TestCase):
    def test_nearest_rank_percentiles_and_spread(self):
        summary = bench.summarize([5, 1, 4, 2, 3, 6, 7, 8, 9, 10])
        self.assertEqual(summary["p50"], 5.5)
        self.assertEqual(summary["p95"], 10)  # ceil(0.95 * 10) = 10th value
        self.assertEqual((summary["min"], summary["max"], summary["spread"]), (1, 10, 9))
        self.assertEqual(summary["n"], 10)

    def test_five_runs_p95_is_the_maximum(self):
        summary = bench.summarize([10, 12, 11, 13, 14])
        self.assertEqual((summary["p50"], summary["p95"], summary["mean"]), (12, 14, 12.0))


class BudgetTests(unittest.TestCase):
    def test_required_speed_for_24_hours(self):
        budget = bench.install_budget(2_000_000.0)
        self.assertEqual(budget["status"], "hypothesis")
        low, high = budget["rows"]
        self.assertEqual(low["required_ips"], round(1e12 / 86400))
        self.assertEqual(high["required_ips"], round(1e13 / 86400))
        self.assertAlmostEqual(low["hours_at_measured"], 1e12 / 2e6 / 3600, places=1)
        self.assertAlmostEqual(high["factor_over_measured"], 1e13 / 86400 / 2e6, delta=0.1)


class NiceWrapperTests(unittest.TestCase):
    def test_wrapper_adds_the_missing_increment(self):
        wrapper, current = bench.nice_wrapper()
        if current == bench.TARGET_NICE:
            self.assertEqual(wrapper, [])
        else:
            self.assertEqual(wrapper, ["nice", "-n", str(bench.TARGET_NICE - current)])


class EngineFactsTests(unittest.TestCase):
    def test_engine_is_derived_from_firmware_path_search_only(self):
        facts = bench.engine_facts()
        self.assertIn("firmware_path_mentions_jit", facts)
        self.assertIn("od_jit_linked_in_runner", facts)
        self.assertNotIn("jit_crate_in_od_isa", facts)
        self.assertIsInstance(facts["firmware_path_mentions_jit"], bool)
        self.assertEqual(facts["firmware_path_mentions_jit"], any(facts["jit_mentions"].values()))
        self.assertEqual(facts["engine"].startswith("interpreter"),
                         bool(facts["interpreter_call"]) and not facts["firmware_path_mentions_jit"])
        self.assertIn("not a proof", facts["engine_basis"])
        note = facts["od_jit_evidence"]["note"]
        if facts["firmware_path_mentions_jit"]:
            self.assertIn("WAS found", note)
        else:
            self.assertIn("no od-jit mention was found", note)

    def test_od_jit_link_is_recorded_separately(self):
        facts = bench.engine_facts()
        self.assertEqual(facts["od_jit_linked_in_runner"], all(facts["od_jit_evidence"]["cargo_manifests"].values()))
        self.assertEqual(set(facts["od_jit_evidence"]["cargo_manifests"]), set(bench.JIT_LINK_MANIFESTS))
        self.assertEqual(set(facts["od_jit_evidence"]["used_in_sources"]), set(bench.JIT_USERS))


class RunOutputTests(unittest.TestCase):
    def test_no_rss_field_is_produced(self):
        import inspect
        self.assertNotIn("max_rss", inspect.getsource(bench))
        self.assertNotIn("ru_maxrss", inspect.getsource(bench))


if __name__ == "__main__":
    unittest.main()
