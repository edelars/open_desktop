"""Guest acceptance regressions for the Stage 6 workloads."""
import gzip
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from build_stage4_initramfs import BuildError, pack_cpio, parse_cpio, sha256
from build_stage6_smp_rootfs_initramfs import (
    FIXTURES,
    PREBUILT_NAME,
    PREBUILT_BUSYBOX_NAME,
    PREBUILT_SOURCE_SHA256,
    SOURCE_NAME,
    STAGE5_HANDOFF,
    STAGE5_SWITCH_ROOT,
    WRAPPER_NAME,
    build_archive,
    validate_prebuilt_workload,
    verify_compute,
    verify_workloads,
)

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


def minimal_amd64_elf() -> bytes:
    header = bytearray(64)
    header[:7] = b"\x7fELF\x02\x01\x01"
    header[16:18] = (3).to_bytes(2, "little")
    header[18:20] = (62).to_bytes(2, "little")
    header[20:24] = (1).to_bytes(4, "little")
    return bytes(header)


def minimal_static_amd64_elf() -> bytes:
    elf = bytearray(120)
    elf[:7] = b"\x7fELF\x02\x01\x01"
    elf[16:18] = (2).to_bytes(2, "little")
    elf[18:20] = (62).to_bytes(2, "little")
    elf[20:24] = (1).to_bytes(4, "little")
    elf[32:40] = (64).to_bytes(8, "little")
    elf[52:54] = (64).to_bytes(2, "little")
    elf[54:56] = (56).to_bytes(2, "little")
    elf[56:58] = (1).to_bytes(2, "little")
    elf[64:68] = (1).to_bytes(4, "little")  # PT_LOAD, with no PT_INTERP/PT_DYNAMIC.
    elf[68:72] = (5).to_bytes(4, "little")
    elf[72:80] = (0).to_bytes(8, "little")
    elf[80:88] = (0x400000).to_bytes(8, "little")
    elf[88:96] = (0x400000).to_bytes(8, "little")
    elf[96:104] = len(elf).to_bytes(8, "little")
    elf[104:112] = len(elf).to_bytes(8, "little")
    elf[112:120] = (0x1000).to_bytes(8, "little")
    return bytes(elf)


def stage5_base(busybox: bytes | None = None) -> bytes:
    init = b"#!/bin/sh\n" + STAGE5_HANDOFF + STAGE5_SWITCH_ROOT
    entries = [("init", 0o100755, init)]
    if busybox is not None:
        entries.append(("bin/busybox", 0o100755, busybox))
    return gzip.compress(pack_cpio(entries), mtime=0)


def overlay_entries(archive: bytes, base: bytes | None = None) -> dict[str, dict[str, object]]:
    base_length = len(gzip.decompress(stage5_base() if base is None else base))
    return {entry["name"].removeprefix("./"): entry for entry in parse_cpio(archive[base_length:])}


class PrebuiltWorkloadPackageTests(unittest.TestCase):
    def test_prebuilt_archive_is_explicit_and_keeps_the_guest_source(self):
        elf = minimal_amd64_elf()
        with patch("build_stage6_smp_rootfs_initramfs.PREBUILT_WORKLOAD_SHA256", sha256(elf)):
            archive = gzip.decompress(build_archive(stage5_base(), elf))
        entries = overlay_entries(archive)
        packaged_elf = entries[f"s6-smp/{PREBUILT_NAME}"]
        self.assertEqual(packaged_elf["mode"] & 0o170000, 0o100000)
        self.assertEqual(packaged_elf["mode"] & 0o777, 0o755)
        self.assertEqual(packaged_elf["data"], elf)
        self.assertEqual(entries[f"s6-smp/{SOURCE_NAME}"]["data"], (FIXTURES / SOURCE_NAME).read_bytes())
        self.assertEqual(sha256(entries[f"s6-smp/{SOURCE_NAME}"]["data"]), PREBUILT_SOURCE_SHA256)
        init = entries["s6-smp-init"]["data"]
        self.assertIn(b"s6_smp_workloads.sh --prebuilt /run/open-desktop-stage6/s6_smp_workloads.elf \"$@\"", init)
        self.assertIn(b"cp /s6-smp/s6_smp_workloads.c /s6-smp/s6_smp_workloads.sh /s6-smp/s6_smp_workloads.elf", init)
        wrapper = entries[f"s6-smp/{WRAPPER_NAME}"]["data"]
        self.assertIn(b"[ \"$prebuilt\" = \"$fixture_dir/s6_smp_workloads.elf\" ] || exit 64", wrapper)
        self.assertIn(b"[ -x \"$prebuilt\" ] || exit 1", wrapper)
        self.assertIn(b"/usr/bin/gcc-15", wrapper)

    def test_default_package_path_does_not_include_or_select_a_prebuilt(self):
        base = stage5_base()
        archive = gzip.decompress(build_archive(base))
        entries = overlay_entries(archive)
        self.assertNotIn(f"s6-smp/{PREBUILT_NAME}", entries)
        self.assertNotIn(b"--prebuilt", entries["s6-smp-init"]["data"])
        self.assertEqual(entries["s6-smp/s6_smp_workloads.sh"]["data"], (FIXTURES / "s6_smp_workloads.sh").read_bytes())

    def test_prebuilt_requires_the_pinned_amd64_elf(self):
        elf = minimal_amd64_elf()
        with patch("build_stage6_smp_rootfs_initramfs.PREBUILT_WORKLOAD_SHA256", sha256(elf)):
            validate_prebuilt_workload(elf, (FIXTURES / SOURCE_NAME).read_bytes())
        invalid_elf = b"not an ELF"
        with patch("build_stage6_smp_rootfs_initramfs.PREBUILT_WORKLOAD_SHA256", sha256(invalid_elf)):
            with self.assertRaises(BuildError):
                validate_prebuilt_workload(invalid_elf, (FIXTURES / SOURCE_NAME).read_bytes())
        wrong_arch = bytearray(elf)
        wrong_arch[18:20] = (183).to_bytes(2, "little")
        with patch("build_stage6_smp_rootfs_initramfs.PREBUILT_WORKLOAD_SHA256", sha256(wrong_arch)):
            with self.assertRaises(BuildError):
                validate_prebuilt_workload(bytes(wrong_arch), (FIXTURES / SOURCE_NAME).read_bytes())
        with self.assertRaises(BuildError):
            validate_prebuilt_workload(elf, b"changed S6-22 source")

    def test_static_prebuilt_requires_static_elf_and_pinned_busybox_for_tmpfs_shell(self):
        workload = minimal_static_amd64_elf()
        busybox = minimal_static_amd64_elf()
        with (
            patch("build_stage6_smp_rootfs_initramfs.PREBUILT_STATIC_WORKLOAD_SHA256", sha256(workload)),
            patch("build_stage6_smp_rootfs_initramfs.PREBUILT_BUSYBOX_SHA256", sha256(busybox)),
        ):
            self.assertEqual(
                validate_prebuilt_workload(workload, (FIXTURES / SOURCE_NAME).read_bytes()),
                "static",
            )
            base = stage5_base()
            archive = gzip.decompress(build_archive(base, workload, busybox))
        entries = overlay_entries(archive, base)
        init = entries["s6-smp-init"]["data"]
        self.assertIn(
            b"cp /s6-smp/s6_smp_busybox /newroot/run/open-desktop-stage6/busybox",
            init,
        )
        self.assertIn(
            b"ln -s ../busybox /newroot/run/open-desktop-stage6/bin/sha256sum",
            init,
        )
        self.assertIn(
            b"ln -s ../busybox /newroot/run/open-desktop-stage6/bin/sleep",
            init,
        )
        self.assertIn(
            b"switch_root /newroot /run/open-desktop-stage6/busybox sh "
            b"/run/open-desktop-stage6/s6_smp_workloads.sh --prebuilt "
            b"/run/open-desktop-stage6/s6_smp_workloads.elf",
            init,
        )
        self.assertEqual(entries[f"s6-smp/{PREBUILT_NAME}"]["data"], workload)
        packaged_busybox = entries[f"s6-smp/{PREBUILT_BUSYBOX_NAME}"]
        self.assertEqual(packaged_busybox["mode"] & 0o777, 0o755)
        self.assertEqual(packaged_busybox["data"], busybox)
        wrapper = entries[f"s6-smp/{WRAPPER_NAME}"]["data"]
        self.assertIn(b"PATH=/run/open-desktop-stage6/bin:/usr/sbin:/usr/bin:/sbin:/bin", wrapper)
        self.assertEqual(
            wrapper.count(b"PATH=/run/open-desktop-stage6/bin:"),
            1,
        )
        self.assertEqual(
            entries[f"s6-smp/{SOURCE_NAME}"]["data"],
            (FIXTURES / SOURCE_NAME).read_bytes(),
        )
        with (
            patch("build_stage6_smp_rootfs_initramfs.PREBUILT_STATIC_WORKLOAD_SHA256", sha256(workload)),
            patch("build_stage6_smp_rootfs_initramfs.PREBUILT_BUSYBOX_SHA256", sha256(busybox)),
        ):
            with self.assertRaisesRegex(BuildError, "requires the pinned static BusyBox"):
                build_archive(base, workload)

    def test_static_wrapper_uses_its_scoped_multicall_applet_path(self):
        source = (FIXTURES / SOURCE_NAME).read_bytes()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload = root / "payload"
            applet_dir = payload / "bin"
            applet_dir.mkdir(parents=True)
            (payload / SOURCE_NAME).write_bytes(source)
            online_path = root / "online"
            online_path.write_text("0-1\n")
            applet_marker = root / "applet-called"
            multicall = applet_dir / "busybox"
            multicall.write_text(
                "#!/usr/bin/env python3\n"
                "import hashlib, os, pathlib, sys\n"
                "path = pathlib.Path(sys.argv[1])\n"
                "pathlib.Path(os.environ['S6_TEST_APPLET_MARKER']).write_text(pathlib.Path(sys.argv[0]).name)\n"
                "print(hashlib.sha256(path.read_bytes()).hexdigest() + '  ' + str(path))\n"
            )
            multicall.chmod(0o755)
            (applet_dir / "sha256sum").symlink_to("busybox")
            (applet_dir / "sleep").symlink_to("busybox")
            wrapper = (FIXTURES / WRAPPER_NAME).read_bytes()
            wrapper = wrapper.replace(b"/run/open-desktop-stage6", os.fsencode(payload))
            wrapper = wrapper.replace(b"/sys/devices/system/cpu/online", os.fsencode(online_path))
            wrapper = wrapper.replace(
                b"PATH=/usr/sbin:/usr/bin:/sbin:/bin",
                b"PATH=" + os.fsencode(applet_dir) + b":/usr/sbin:/usr/bin:/sbin:/bin",
                1,
            )
            wrapper_path = root / WRAPPER_NAME
            wrapper_path.write_bytes(wrapper)
            workload_path = payload / PREBUILT_NAME
            workload_path.write_text(
                "#!/bin/sh\n"
                "nonce=$1\n"
                "printf 'OD_S6_22_PASS nonce=%s case=counter cpus=0,1 checked=20000\\n' \"$nonce\"\n"
                "printf 'OD_S6_22_PASS nonce=%s case=condition cpus=0,1 checked=256\\n' \"$nonce\"\n"
                "printf 'OD_S6_22_PASS nonce=%s case=exchange cpus=0,1 checked=128\\n' \"$nonce\"\n"
                "printf 'OD_S6_22_PASS nonce=%s case=mapping cpus=0,1 checked=32\\n' \"$nonce\"\n"
                "printf 'OD_S6_22_PASS nonce=%s case=processes cpus=0,1 checked=32\\n' \"$nonce\"\n"
            )
            workload_path.chmod(0o755)
            result = subprocess.run(
                ["/bin/sh", str(wrapper_path), "--prebuilt", str(workload_path), "static-wrapper"],
                check=False,
                capture_output=True,
                text=True,
                env={**os.environ, "PATH": "/usr/bin:/bin", "S6_TEST_APPLET_MARKER": str(applet_marker)},
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(applet_marker.read_text(), "sha256sum")
            self.assertIn("OD_S6_22_COMPLETE nonce=static-wrapper", result.stdout)
            transcript = "[    1.71] smp: Brought up 1 node, 2 CPUs\r\n" + result.stdout
            report = root / "report.json"
            report.write_text(json.dumps({"stop": {"com1_text": transcript}}))
            self.assertEqual(verify_workloads(report, "static-wrapper", 2)["workload_cpus"], (0, 1))

    def test_static_prebuilt_rejects_dynamic_program_headers_and_changed_digests(self):
        workload = minimal_static_amd64_elf()
        for segment_type in (2, 3):  # PT_DYNAMIC and PT_INTERP.
            non_static = bytearray(workload)
            non_static[64:68] = segment_type.to_bytes(4, "little")
            with patch(
                "build_stage6_smp_rootfs_initramfs.PREBUILT_STATIC_WORKLOAD_SHA256",
                sha256(non_static),
            ):
                with self.assertRaisesRegex(BuildError, "PT_LOAD and no PT_INTERP or PT_DYNAMIC"):
                    validate_prebuilt_workload(bytes(non_static), (FIXTURES / SOURCE_NAME).read_bytes())
        modified = bytearray(workload)
        modified[-1] ^= 1
        with patch(
            "build_stage6_smp_rootfs_initramfs.PREBUILT_STATIC_WORKLOAD_SHA256",
            sha256(workload),
        ):
            with self.assertRaisesRegex(BuildError, "SHA-256"):
                validate_prebuilt_workload(bytes(modified), (FIXTURES / SOURCE_NAME).read_bytes())
        with (
            patch("build_stage6_smp_rootfs_initramfs.PREBUILT_STATIC_WORKLOAD_SHA256", sha256(workload)),
        ):
            with self.assertRaisesRegex(BuildError, "BusyBox SHA-256"):
                build_archive(stage5_base(), workload, b"wrong busybox")

    def test_shared_wrapper_compile_and_explicit_prebuilt_branches_verify(self):
        source = (FIXTURES / SOURCE_NAME).read_bytes()
        workload = (
            "#!/bin/sh\n"
            "nonce=$1\n"
            "if [ \"$#\" -eq 5 ] && [ \"$2\" = --compute ]; then\n"
            "  workers=$3; blocks=$4; checksum=$5\n"
            "  printf 'OD_S6_38_BEGIN nonce=%s workers=%s blocks=%s\\n' \"$nonce\" \"$workers\" \"$blocks\"\n"
            "  printf 'OD_S6_38_END nonce=%s\\n' \"$nonce\"\n"
            "  printf 'OD_S6_38_COMPLETE nonce=%s workers=%s blocks=%s checksum=%s\\n' \"$nonce\" \"$workers\" \"$blocks\" \"$checksum\"\n"
            "else\n"
            "  printf 'OD_S6_22_PASS nonce=%s case=counter cpus=0,1 checked=20000\\n' \"$nonce\"\n"
            "  printf 'OD_S6_22_PASS nonce=%s case=condition cpus=0,1 checked=256\\n' \"$nonce\"\n"
            "  printf 'OD_S6_22_PASS nonce=%s case=exchange cpus=0,1 checked=128\\n' \"$nonce\"\n"
            "  printf 'OD_S6_22_PASS nonce=%s case=mapping cpus=0,1 checked=32\\n' \"$nonce\"\n"
            "  printf 'OD_S6_22_PASS nonce=%s case=processes cpus=0,1 checked=32\\n' \"$nonce\"\n"
            "fi\n"
        ).encode()
        for prebuilt in (False, True):
            with self.subTest(prebuilt=prebuilt), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                payload = root / "payload"
                payload.mkdir()
                (payload / SOURCE_NAME).write_bytes(source)
                online_path = root / "online"
                online_path.write_text("0-1\n")
                wrapper_path = root / WRAPPER_NAME
                wrapper = (FIXTURES / WRAPPER_NAME).read_bytes()
                wrapper = wrapper.replace(b"/run/open-desktop-stage6", os.fsencode(payload))
                wrapper = wrapper.replace(b"/sys/devices/system/cpu/online", os.fsencode(online_path))
                compiler_path = payload / "test-gcc-15"
                compiler_path.write_text(
                    "#!/bin/sh\n"
                    "out=\nwhile [ \"$#\" -gt 0 ]; do\n"
                    "  if [ \"$1\" = -o ]; then out=$2; shift 2; else shift; fi\n"
                    "done\n"
                    "cp \"$S6_TEST_WORKLOAD_STUB\" \"$out\"\n"
                    "chmod +x \"$out\"\n"
                    "printf compiled > \"$S6_TEST_COMPILER_MARKER\"\n"
                )
                compiler_path.chmod(0o755)
                wrapper = wrapper.replace(b"/usr/bin/gcc-15", os.fsencode(compiler_path))
                wrapper_path.write_bytes(wrapper)
                wrapper_path.chmod(0o755)
                workload_path = payload / PREBUILT_NAME
                if prebuilt:
                    workload_path.write_bytes(workload)
                    workload_path.chmod(0o755)
                stub_path = root / "workload-stub"
                stub_path.write_bytes(workload)
                stub_path.chmod(0o755)
                compiler_marker = root / "compiler-called"
                nonce = "s6-wrapper-prebuilt" if prebuilt else "s6-wrapper-compile"
                command = ["/bin/sh", str(wrapper_path)]
                if prebuilt:
                    command.extend(["--prebuilt", str(workload_path)])
                command.append(nonce)
                result = subprocess.run(
                    command,
                    check=False,
                    capture_output=True,
                    text=True,
                    env={
                        **os.environ,
                        "S6_TEST_WORKLOAD_STUB": str(stub_path),
                        "S6_TEST_COMPILER_MARKER": str(compiler_marker),
                    },
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(compiler_marker.exists(), not prebuilt)
                if prebuilt:
                    wrong_path = subprocess.run(
                        ["/bin/sh", str(wrapper_path), "--prebuilt", str(payload / "unrelated.elf"), nonce],
                        check=False,
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(wrong_path.returncode, 64)
                transcript = "[    1.71] smp: Brought up 1 node, 2 CPUs\r\n" + result.stdout
                report = root / "report.json"
                report.write_text(json.dumps({"stop": {"com1_text": transcript}}))
                self.assertEqual(verify_workloads(report, nonce, 2)["online_cpus"], [0, 1])
                if prebuilt:
                    checksum = "ec4675d8578a4e14"
                    compute_nonce = "s6-wrapper-compute"
                    compute = subprocess.run(
                        [
                            "/bin/sh", str(wrapper_path), "--prebuilt", str(workload_path),
                            compute_nonce, "--compute", f"2,4097,{checksum}",
                        ],
                        check=False, capture_output=True, text=True,
                        env={
                            **os.environ,
                            "S6_TEST_WORKLOAD_STUB": str(stub_path),
                            "S6_TEST_COMPILER_MARKER": str(compiler_marker),
                        },
                    )
                    self.assertEqual(compute.returncode, 0, compute.stderr)
                    compute_report = root / "compute-report.json"
                    compute_report.write_text(
                        json.dumps({"stop": {"com1_text": "[    1.71] smp: Brought up 1 node, 2 CPUs\r\n" + compute.stdout}})
                    )
                    self.assertEqual(
                        verify_compute(compute_report, compute_nonce, 2, f"2,4097,{checksum}")["checksum"],
                        checksum,
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
