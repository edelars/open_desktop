from __future__ import annotations

import importlib.util
import json
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
CPU = ROOT / "tests/cpu"
sys.path.insert(0, str(CPU))
SPEC = importlib.util.spec_from_file_location(
    "stage2_42_linux_five_suites", CPU / "run_stage2_42_linux_five_suites.py"
)
assert SPEC is not None and SPEC.loader is not None
aggregator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(aggregator)


class CombinedAcceptanceTests(unittest.TestCase):
    def test_aggregate_names_exact_selected_and_unselected_sets(self):
        fingerprint = "a" * 64
        children = {
            suite: {"status": "passed", "source_fingerprint": fingerprint,
                    "case_count": aggregator.EXPECTED_CASES[suite], "repeat_count": 2}
            for suite in aggregator.SUITE_IDS
        }
        summary = aggregator.validate_aggregate(children, fingerprint)
        self.assertEqual(summary["status"], "passed")
        self.assertEqual(summary["selection"]["selected_suites"], list(aggregator.SUITE_IDS))
        self.assertEqual(summary["selection"]["unselected_suites"],
                         list(aggregator.UNSELECTED_SUITES))
        self.assertEqual(summary["physical_mac_acceptance"], "not_run")
        self.assertEqual(summary["parent_s2_42_status"], "open")

    def test_aggregate_rejects_missing_failed_or_stale_required_child(self):
        fingerprint = "b" * 64
        children = {
            suite: {"status": "passed", "source_fingerprint": fingerprint,
                    "case_count": aggregator.EXPECTED_CASES[suite], "repeat_count": 2}
            for suite in aggregator.SUITE_IDS
        }
        with self.assertRaisesRegex(aggregator.AcceptanceError, "missing"):
            aggregator.validate_aggregate({key: value for key, value in children.items()
                                           if key != "S2-42r"}, fingerprint)
        children["S2-42r"]["source_fingerprint"] = "c" * 64
        with self.assertRaisesRegex(aggregator.AcceptanceError, "stale"):
            aggregator.validate_aggregate(children, fingerprint)
        children["S2-42r"]["source_fingerprint"] = fingerprint
        children["S2-42r"]["status"] = "failed"
        with self.assertRaisesRegex(aggregator.AcceptanceError, "required aggregate check"):
            aggregator.validate_aggregate(children, fingerprint)

    def test_child_verification_binds_manifest_to_current_source(self):
        fingerprint = "d" * 64
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp)
            run = output / "run"
            run.mkdir()
            (run / "manifest.json").write_text(json.dumps({
                "status": "passed", "suite": "xorpd-native-corpus",
                "source_fingerprint": fingerprint, "case_counts": {"total": 1030}, "repeats": 2,
            }))
            child = aggregator.validate_child("S2-42c", output, 0, fingerprint)
            self.assertEqual(child["case_count"], 1030)
            with self.assertRaisesRegex(aggregator.AcceptanceError, "stale"):
                aggregator.validate_child("S2-42c", output, 0, "e" * 64)

    def test_child_verification_uses_mov_total_count_and_strict_summary(self):
        fingerprint = "f" * 64
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp)
            run = output / "run"
            run.mkdir()
            manifest = run / "manifest.json"
            manifest.write_text(json.dumps({"status": "passed", "source_fingerprint": fingerprint}))
            (output / "s2-42e-summary.json").write_text(json.dumps({
                "status": "passed", "total_count": 1080, "repeat_count": 2,
                "physical_mac_acceptance": "not_run", "oracle_manifest": str(manifest),
            }))
            child = aggregator.validate_child("S2-42e", output, 0, fingerprint)
            self.assertEqual(child["case_count"], 1080)
            bad = json.loads((output / "s2-42e-summary.json").read_text())
            bad["physical_mac_acceptance"] = "passed"
            (output / "s2-42e-summary.json").write_text(json.dumps(bad))
            with self.assertRaisesRegex(aggregator.AcceptanceError, "must remain not_run"):
                aggregator.validate_child("S2-42e", output, 0, fingerprint)

    def test_inside_work_rejects_work_symlink_outside_repository(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            root = base / "repo"
            root.mkdir()
            outside = base / "outside"
            outside.mkdir()
            (root / "work").symlink_to(outside, target_is_directory=True)
            with patch.object(aggregator, "ROOT", root):
                with self.assertRaisesRegex(aggregator.AcceptanceError, "outside the repository"):
                    aggregator._inside_work(root / "work" / "results")

    @staticmethod
    def _process_tree_command(ready: Path, marker: Path, *, ignore_term: bool = False,
                              redirect_child_output: bool = False) -> list[str]:
        child_setup = "import pathlib,time; "
        if ignore_term:
            child_setup += "import signal; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
        child_code = (child_setup + f"pathlib.Path({str(ready)!r}).write_text('ready'); "
                      + "time.sleep(2.5); "
                      f"pathlib.Path({str(marker)!r}).write_text('survived')")
        child_output = ",stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL" \
            if redirect_child_output else ""
        parent_code = (
            "import pathlib,subprocess,sys,time; "
            f"subprocess.Popen([sys.executable,'-c',{child_code!r}]{child_output}); "
            "time.sleep(30)"
        )
        return [sys.executable, "-c", parent_code]

    def test_timeout_terminates_grandchild_process_group(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            ready, marker = directory / "ready", directory / "survived"
            process, timed_out = aggregator._run_child_command(
                self._process_tree_command(ready, marker, ignore_term=True),
                cwd=directory, timeout=0.2
            )
            self.assertTrue(timed_out)
            self.assertEqual(process.returncode, 124)
            time.sleep(1.1)
            self.assertFalse(marker.exists(), "grandchild continued after parent timeout")

    def test_timeout_kills_term_ignoring_grandchild_with_redirected_output(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            ready, marker = directory / "ready", directory / "survived"
            command = self._process_tree_command(
                ready, marker, ignore_term=True, redirect_child_output=True
            )
            process, timed_out = aggregator._run_child_command(command, cwd=directory, timeout=0.2)
            self.assertTrue(timed_out)
            self.assertEqual(process.returncode, 124)
            time.sleep(1.1)
            self.assertFalse(marker.exists(), "TERM-ignoring grandchild survived timeout cleanup")

    def test_interrupt_terminates_grandchild_process_group(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            ready, marker = directory / "ready", directory / "survived"
            command = self._process_tree_command(
                ready, marker, ignore_term=True, redirect_child_output=True
            )
            real_popen = subprocess.Popen

            class InterruptingPopen:
                def __init__(self, *args, **kwargs):
                    self.inner = real_popen(*args, **kwargs)
                    self.interrupted = False

                def __getattr__(self, name):
                    return getattr(self.inner, name)

                def communicate(self, timeout=None):
                    if not self.interrupted:
                        deadline = time.monotonic() + 3
                        while not ready.exists() and time.monotonic() < deadline:
                            time.sleep(0.01)
                        if not ready.exists():
                            raise AssertionError("child process did not reach the ready state")
                        self.interrupted = True
                        raise KeyboardInterrupt
                    return self.inner.communicate(timeout=timeout)

            with patch.object(aggregator.subprocess, "Popen", InterruptingPopen):
                with self.assertRaises(KeyboardInterrupt):
                    aggregator._run_child_command(command, cwd=directory, timeout=10)
            time.sleep(1.1)
            self.assertFalse(marker.exists(), "TERM-ignoring grandchild survived interrupt cleanup")

    def test_second_interrupt_still_sends_group_kill(self):
        class ProcessStub:
            pid = 12345

            def communicate(self, timeout=None):
                return "", ""

        signals = []
        with patch.object(aggregator.os, "killpg",
                          side_effect=lambda pgid, sig: signals.append((pgid, sig))), \
                patch.object(aggregator.time, "sleep", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                aggregator._terminate_process_group(ProcessStub())
        self.assertEqual(signals, [(12345, signal.SIGTERM), (12345, signal.SIGKILL)])

    def test_group_kill_permission_error_is_not_suppressed(self):
        class ProcessStub:
            pid = 12346

            def communicate(self, timeout=None):
                return "", ""

        with patch.object(aggregator.os, "killpg", side_effect=[None, PermissionError("denied")]), \
                patch.object(aggregator.time, "sleep"):
            with self.assertRaisesRegex(PermissionError, "denied"):
                aggregator._terminate_process_group(ProcessStub())


if __name__ == "__main__":
    unittest.main()
