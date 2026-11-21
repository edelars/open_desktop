#!/usr/bin/env python3
"""Self-test of tools/build/firmware_check_all.py: coverage of smoke scripts and key lines."""

from __future__ import annotations

from pathlib import Path
import unittest

import firmware_check_all as check_all


class FirmwareCheckAllTest(unittest.TestCase):
    def test_every_firmware_smoke_script_has_a_step(self):
        on_disk = {path.name for path in (check_all.ROOT / "tests" / "system").glob("firmware_*_smoke.py")}
        covered = {step.script for step in check_all.steps() if step.script}
        self.assertEqual(covered, on_disk)

    def test_prerequisites_name_earlier_steps(self):
        seen = set()
        for step in check_all.steps():
            self.assertTrue(set(step.needs) <= seen, step.name)
            seen.add(step.name)
        self.assertEqual(len(seen), len(check_all.steps()))

    def test_key_line_prefers_cargo_summary_and_failure_marker(self):
        log = "running 1 test\ntest result: ok. 1 passed; 0 failed\n\nDoc-tests x\n"
        self.assertEqual(check_all.key_line(log, False), "test result: ok. 1 passed; 0 failed")
        failing = "step\nthread 'x' panicked at y\nSTEP FAILED: exit 101\n"
        self.assertEqual(check_all.key_line(failing, True), "thread 'x' panicked at y")
        self.assertEqual(check_all.key_line("a\nlast\n", False), "last")
        self.assertEqual(check_all.key_line("", False), "(no output)")

    def test_table_has_one_line_per_row_plus_header(self):
        text = check_all.table([("a", "0", "1", "ok"), ("bb", "2", "10", "bad")])
        self.assertEqual(len(text.splitlines()), 3)
        self.assertIn("bad", text)


if __name__ == "__main__":
    unittest.main()
