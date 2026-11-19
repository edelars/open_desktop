"""Guest acceptance regressions for the Stage 6 workloads."""
import json
import tempfile
import unittest
from pathlib import Path

from build_stage4_initramfs import BuildError, sha256
from build_stage6_smp_rootfs_initramfs import FIXTURES, SOURCE_NAME, verify_compute, verify_workloads

NONCE = "s6-workload-check"


def guest_transcript(online: int = 2) -> str:
    source = sha256((FIXTURES / SOURCE_NAME).read_bytes())
    return (
        f"[    1.71] smp: Brought up 1 node, {online} CPUs\r\n"
        f"OD_S6_22_SOURCE nonce={NONCE} sha256={source}\r\n"
        f"OD_S6_22_PASS nonce={NONCE} case=counter cpus=0,1 checked=20000\r\n"
        f"OD_S6_22_PASS nonce={NONCE} case=condition cpus=0,1 checked=256\r\n"
        f"OD_S6_22_PASS nonce={NONCE} case=exchange cpus=0,1 checked=128\r\n"
        f"OD_S6_22_PASS nonce={NONCE} case=mapping cpus=0,1 checked=32\r\n"
        f"OD_S6_22_PASS nonce={NONCE} case=processes cpus=0,1 checked=32\r\n"
        f"OD_S6_22_ONLINE nonce={NONCE} cpus=0-{online - 1}\r\n"
        f"OD_S6_22_COMPLETE nonce={NONCE}\r\n"
    )


class SmpWorkloadReportTests(unittest.TestCase):
    def verify(self, transcript: str, online: int = 2):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.json"
            report.write_text(json.dumps({"stop": {"com1_text": transcript}}))
            return verify_workloads(report, NONCE, online)

    def test_all_five_workloads_are_bound_to_a_kernel_boot_and_source(self):
        self.assertEqual(self.verify(guest_transcript())["workload_cpus"], (0, 1))
        transcript = guest_transcript(4).replace("cpus=0,1", "cpus=2,3")
        transcript += "[    2.50] watchdog: Hard lockup detector is disabled\r\n"
        observed = self.verify(transcript, 4)
        self.assertEqual(observed["workload_cpus"], (2, 3))
        self.assertEqual(observed["online_cpus"], [0, 1, 2, 3])
        self.assertEqual(
            observed["checked"],
            {"counter": 20000, "condition": 256, "exchange": 128, "mapping": 32, "processes": 32},
        )

    def test_incomplete_or_inconsistent_guest_runs_are_refused(self):
        good = guest_transcript()
        rows = good.splitlines()
        invalid = {
            "missing kernel summary": "\n".join(rows[1:]),
            "late kernel summary": "\n".join(rows[1:] + rows[:1]),
            "wrong SMP count": good.replace("2 CPUs", "4 CPUs"),
            "source digest": good.replace("sha256=", "sha256=0"),
            "other nonce": good.replace("nonce=" + NONCE, "nonce=wrong", 1),
            "incorrect count": good.replace("checked=20000", "checked=19999"),
            "duplicate CPU": good.replace("cpus=0,1", "cpus=0,0", 1),
            "offline CPU": good.replace("cpus=0,1", "cpus=0,2", 1),
            "CPU offlined after boot": good.replace("cpus=0-1", "cpus=0"),
            "wrong online marker nonce": good.replace(f"ONLINE nonce={NONCE}", "ONLINE nonce=other"),
            "wrong case": good.replace("case=processes", "case=mapping"),
            "incomplete": "\n".join(rows[:-1]),
            "duplicate completion": good + rows[-1],
            "wrapper failure": good + f"OD_S6_22_WRAPPER_FAIL nonce={NONCE} step=run rc=1",
            "kernel panic": good + "[    2.50] Kernel panic - not syncing: stopped",
            "kernel stall": good + "[    2.50] rcu: INFO: rcu_preempt detected stalls on CPUs/tasks",
            "kernel lockup": good + "[    2.50] watchdog: Watchdog detected hard LOCKUP on cpu 1",
            "hung task": good + "[    2.50] INFO: task kworker/1:0:23 blocked for more than 120 seconds.",
            "kernel oops": good + "[    2.50] Oops: general protection fault, probably for non-canonical address",
            "kernel APIC mismatch": good + "[Firmware Bug]: CPUID leaf 0xb subleaf 0 APIC ID mismatch 0 != 1",
        }
        for label, transcript in invalid.items():
            with self.subTest(label=label), self.assertRaises(BuildError):
                self.verify(transcript)


class ComputeReportTests(unittest.TestCase):
    checksum = "ec4675d8578a4e14"

    def transcript(self, online: int, workers: int) -> str:
        source = sha256((FIXTURES / SOURCE_NAME).read_bytes())
        summary = "1 CPU" if online == 1 else f"{online} CPUs"
        cpus = "0" if online == 1 else f"0-{online - 1}"
        return (
            f"[    1.71] smp: Brought up 1 node, {summary}\r\n"
            f"OD_S6_38_SOURCE nonce={NONCE} sha256={source}\r\n"
            f"OD_S6_38_BEGIN nonce={NONCE} workers={workers} blocks=4097\r\n"
            f"OD_S6_38_END nonce={NONCE}\r\n"
            f"OD_S6_38_COMPLETE nonce={NONCE} workers={workers} blocks=4097 checksum={self.checksum}\r\n"
            f"OD_S6_38_ONLINE nonce={NONCE} cpus={cpus}\r\n"
        )

    def verify(self, transcript: str, online: int, compute: str):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.json"
            report.write_text(json.dumps({"stop": {"com1_text": transcript}}))
            return verify_compute(report, NONCE, online, compute)

    def test_one_worker_can_be_measured_with_other_cpus_online(self):
        for online, workers in [(1, 1), (2, 2), (4, 1), (4, 4)]:
            with self.subTest(online=online, workers=workers):
                result = self.verify(
                    self.transcript(online, workers), online, f"{workers},4097,{self.checksum.upper()}"
                )
                self.assertEqual(result["online_cpus"], list(range(online)))
                self.assertEqual(result["workers"], workers)
                self.assertEqual(result["blocks"], 4097)
                self.assertEqual(result["checksum"], self.checksum)

    def test_compute_requires_complete_matching_ordered_guest_evidence(self):
        good = self.transcript(4, 2)
        boot, source, begin, end, complete, online = good.splitlines()
        invalid = {
            "repeat": good + complete,
            "late boot": "\n".join([source, boot, begin, end, complete, online]),
            "late begin": "\n".join([boot, source, end, begin, complete, online]),
            "incomplete": "\n".join([boot, source, begin, end, online]),
            "missing online": "\n".join([boot, source, begin, end, complete]),
            "thread count": good.replace("workers=2", "workers=1"),
            "checksum": good.replace(self.checksum, "0" * 16),
            "nonce": good.replace(end, end.replace(NONCE, "wrong")),
            "source digest": good.replace("sha256=", "sha256=0"),
            "online count": good.replace("cpus=0-3", "cpus=0-1"),
            "wrapper failure": good + f"OD_S6_38_WRAPPER_FAIL nonce={NONCE} step=online rc=1",
            "kernel oops": good + "Oops: general protection fault",
        }
        for label, transcript in invalid.items():
            with self.subTest(label=label), self.assertRaises(BuildError):
                self.verify(transcript, 4, f"2,4097,{self.checksum}")
        for args in ["2,1", f"2,1,{self.checksum}", f"2,{2**64},{self.checksum}", f"2,4097,0x{self.checksum}"]:
            with self.subTest(args=args), self.assertRaises(BuildError):
                self.verify(good, 4, args)
