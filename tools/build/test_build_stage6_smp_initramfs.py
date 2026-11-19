"""Build and transcript regressions for the S6-21 guest observer."""

import json
import tempfile
import unittest
from pathlib import Path

from build_stage4_initramfs import INIT_MODE, check_elf, parse_cpio
from build_stage6_smp_initramfs import BuildError, build_archive, parse_observation, verify_report

NONCE = "Stage6ObservationTest"


def two_cpu_transcript() -> str:
    return (
        f"boot output\r\nOD_SMP_V1 {NONCE}\r\n"
        "OD_CPU_ONLINE 0-1\r\nOD_CPU_COUNT 2\r\nOD_IRQ_BEGIN\r\n"
        "            CPU0       CPU1\r\n"
        "  0:           1          2  IO-APIC  timer\r\n"
        "RES:           1          0  Rescheduling interrupts\r\n"
        "CAL:           2          3  Function call interrupts\r\n"
        "OD_IRQ_END\r\nOD_SMP_END\r\n"
    )


class SmpInitramfsTests(unittest.TestCase):
    def test_build_is_reproducible_and_contains_one_static_init(self):
        archive = build_archive()
        self.assertEqual(archive, build_archive())
        (entry,) = parse_cpio(archive)
        self.assertEqual((entry["name"], entry["mode"], entry["mtime"]), ("init", INIT_MODE, 0))
        executable = entry["data"]
        self.assertEqual(
            set(check_elf(executable)["syscalls_used"]),
            {"mkdir", "mount", "openat", "read", "close", "write", "nanosleep"},
        )
        self.assertIn(b"/sys/devices/system/cpu/online\0", executable)
        self.assertIn(b"/proc/interrupts\0", executable)

    def test_parser_keeps_guest_cpu_order_and_raw_irq_lines(self):
        result = parse_observation(two_cpu_transcript(), NONCE)
        self.assertEqual(result["cpus"], [0, 1])
        self.assertEqual(result["ipi_counts"], {"RES": [1, 0], "CAL": [2, 3]})
        self.assertEqual(result["interrupts"][1], "  0:           1          2  IO-APIC  timer")

    def test_up_observation_does_not_require_ipi_rows(self):
        transcript = (
            f"OD_SMP_V1 {NONCE}\nOD_CPU_ONLINE 0\nOD_CPU_COUNT 1\nOD_IRQ_BEGIN\n"
            "   CPU0\n0: 12 XT-PIC timer\nOD_IRQ_END\nOD_SMP_END\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.json"
            report.write_text(json.dumps({"stop": {"com1_text": transcript}}))
            result = verify_report(report, NONCE, 1)
            self.assertEqual(result["cpus"], [0])
            self.assertEqual(result["ipi_counts"], {})
            with self.assertRaisesRegex(BuildError, "expected online CPUs"):
                verify_report(report, NONCE, 2)

    def test_parser_rejects_incomplete_or_inconsistent_evidence(self):
        valid = two_cpu_transcript()
        cases = {
            "truncated": valid.replace("OD_SMP_END\r\n", ""),
            "duplicate": valid + valid,
            "wrong nonce": valid.replace(NONCE, "UnrelatedObservation"),
            "guest error": valid + "OD_SMP_ERROR write\n",
            "overlap": valid.replace("OD_CPU_ONLINE 0-1", "OD_CPU_ONLINE 0-1,1"),
            "count": valid.replace("OD_CPU_COUNT 2", "OD_CPU_COUNT 1"),
            "header": valid.replace("CPU0       CPU1", "CPU0       CPU2"),
            "extra count": valid.replace("3  Function call", "3  4  Function call"),
        }
        for name, transcript in cases.items():
            with self.subTest(name=name), self.assertRaises(BuildError):
                parse_observation(transcript, NONCE)

    def test_smp_verification_requires_matching_kernel_summary_and_ipis(self):
        summary = "[    1.234500] smp: Brought up 1 node, 2 CPUs\r\n"
        valid = summary + two_cpu_transcript()
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.json"
            report.write_text(json.dumps({"stop": {"com1_text": valid}}))
            self.assertEqual(verify_report(report, NONCE, 2, require_smp_evidence=True)["cpus"], [0, 1])
        cases = {
            "missing summary": two_cpu_transcript(),
            "late summary": two_cpu_transcript() + summary,
            "duplicate summary": summary + valid,
            "wrong count": valid.replace("2 CPUs", "1 CPU"),
            "IPI missing on AP": valid.replace("3  Function call", "0  Function call"),
            "no IPI rows": "\r\n".join(
                line for line in valid.splitlines() if not line.startswith(("RES:", "CAL:"))
            ),
        }
        for name, transcript in cases.items():
            with self.subTest(name=name), self.assertRaises(BuildError):
                parse_observation(transcript, NONCE, require_smp_evidence=True)


if __name__ == "__main__":
    unittest.main()
