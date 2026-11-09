#!/usr/bin/env python3
"""Tests for build_stage4_initramfs.py (S4-36). No kernel is needed."""

from __future__ import annotations

import contextlib
import hashlib
import io
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_stage4_initramfs as b  # noqa: E402
from build_stage4_initramfs import BuildError  # noqa: E402

GOOD_CONFIG = "\n".join(
    [
        "CONFIG_BLK_DEV_INITRD=y",
        "CONFIG_X86_64=y",
        "CONFIG_BINFMT_ELF=y",
        "CONFIG_PROC_FS=y",
        "CONFIG_TMPFS=y",
        "CONFIG_RD_GZIP=y",
        "# CONFIG_INITRAMFS_FORCE is not set",
        "",
    ]
)


def fresh_init_elf() -> bytes:
    with tempfile.TemporaryDirectory(prefix="stage4-initramfs-test-") as tmp:
        elf, _versions = b.compile_init(Path(tmp))
    return elf


def run_main(argv: list[str]) -> tuple[int, str]:
    err = io.StringIO()
    out = io.StringIO()
    with contextlib.redirect_stderr(err), contextlib.redirect_stdout(out):
        rc = b.main(argv)
    return rc, err.getvalue()


class InitramfsTestBase(unittest.TestCase):
    elf: bytes

    @classmethod
    def setUpClass(cls) -> None:
        try:
            cls.elf = fresh_init_elf()
        except BuildError as exc:
            raise unittest.SkipTest(f"toolchain unavailable: {exc}") from exc

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="stage4-initramfs-case-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.config = self.tmp / "config"
        self.config.write_text(GOOD_CONFIG)

    def build_with(self, init: bytes, **kwargs):
        init_path = self.tmp / "fake-init"
        init_path.write_bytes(init)
        return b.run_build(
            self.tmp / "out",
            init_binary=init_path,
            config_path=self.config,
            check_kernel_identity=False,
            **kwargs,
        )

    def assertRefused(self, init: bytes, fragment: str) -> None:
        with self.assertRaises(BuildError) as ctx:
            self.build_with(init)
        self.assertIn(fragment, str(ctx.exception))
        self.assertFalse((self.tmp / "out" / "initramfs.cpio").exists(), "no archive may be written")


class PositiveTests(InitramfsTestBase):
    def test_positive_build_passes_all_checks_and_is_repeatable(self) -> None:
        evidence_a = self.tmp / "a.json"
        evidence_b = self.tmp / "b.json"
        init_path = self.tmp / "init"
        init_path.write_bytes(self.elf)
        results = []
        for name, evidence in (("out-a", evidence_a), ("out-b", evidence_b)):
            results.append(
                b.run_build(
                    self.tmp / name,
                    init_binary=init_path,
                    config_path=self.config,
                    evidence_path=evidence,
                    check_kernel_identity=False,
                )
            )
        cpio_a = (self.tmp / "out-a" / "initramfs.cpio").read_bytes()
        cpio_b = (self.tmp / "out-b" / "initramfs.cpio").read_bytes()
        self.assertEqual(cpio_a, cpio_b)
        self.assertEqual(results[0]["cpio"]["sha256"], hashlib.sha256(cpio_a).hexdigest())
        self.assertEqual(results[0]["cpio"]["sha256"], results[1]["cpio"]["sha256"])
        self.assertEqual(results[0]["status"], "passed")
        self.assertEqual([e["name"] for e in results[0]["cpio"]["entries"]], ["/init"])
        self.assertEqual(results[0]["cpio"]["entries"][0]["mode"], "0100755")
        self.assertTrue(results[0]["checks"]["nonce_not_baked"])
        self.assertEqual(len(results[0]["negatives"]), 12)
        self.assertTrue(all(n["rejected"] for n in results[0]["negatives"]))
        for negative in results[0]["negatives"]:
            self.assertNotIn("exit_code", negative)
            self.assertTrue(negative["expected_fragment"])
            self.assertIn(negative["expected_fragment"], negative["message"])
        # a prebuilt --init-binary is one input: no claim of two independent compilations
        self.assertEqual(results[0]["checks"]["repeatable_build"]["runs"], 1)
        self.assertIn("one input", results[0]["init_source"]["build"])
        # evidence is deterministic and carries no timestamps
        self.assertEqual(evidence_a.read_text(), evidence_b.read_text())
        text = evidence_a.read_text()
        for word in ("timestamp", "generated_at", "created_at", "date"):
            self.assertNotIn(word, json.loads(text).keys())
        self.assertNotIn(str(b.REPO_ROOT), text)
        self.assertNotIn(str(self.tmp), text)

    def test_two_independent_compilations_are_identical(self) -> None:
        self.assertEqual(self.elf, fresh_init_elf())

    def test_elf_abi_report(self) -> None:
        report = b.check_elf(self.elf)
        self.assertEqual(report["e_type"], "ET_EXEC")
        self.assertEqual(report["e_machine"], "EM_X86_64")
        self.assertFalse(report["has_pt_interp"] or report["has_pt_dynamic"])
        self.assertEqual(
            sorted(report["syscalls_used"]), ["close", "exit", "getpid", "mkdir", "mount", "nanosleep", "openat", "read", "write"]
        )
        for marker in (b"/Users", b"/home", b"/private", b"/opt"):
            self.assertNotIn(marker, self.elf)

    def test_cpio_layout_is_newc_and_aligned(self) -> None:
        archive = b.build_archive(self.elf)
        self.assertEqual(archive[:6], b"070701")
        self.assertEqual(len(archive) % 4, 0)
        entries = b.parse_cpio(archive)
        self.assertEqual([e["name"] for e in entries], ["init"])
        self.assertEqual((entries[0]["uid"], entries[0]["gid"], entries[0]["mtime"]), (0, 0, 0))
        self.assertIn(b"TRAILER!!!\0", archive)

    @unittest.skipUnless(Path("/usr/bin/cpio").exists(), "/usr/bin/cpio is missing")
    def test_system_cpio_extracts_the_archive(self) -> None:
        archive = self.tmp / "a.cpio"
        archive.write_bytes(b.build_archive(self.elf))
        target = self.tmp / "x"
        target.mkdir()
        with archive.open("rb") as handle:
            subprocess.run(["/usr/bin/cpio", "-id"], stdin=handle, cwd=target, check=True, capture_output=True)
        self.assertEqual((target / "init").read_bytes(), self.elf)
        self.assertTrue(os.access(target / "init", os.X_OK))

    def test_nonce_given_and_not_baked(self) -> None:
        evidence = self.build_with(self.elf, nonce="AbCd1234EfGh5678")
        self.assertTrue(evidence["checks"]["nonce_not_baked"])
        self.assertNotIn(b"AbCd1234EfGh5678", (self.tmp / "out" / "initramfs.cpio").read_bytes())

    def test_kernel_config_checks(self) -> None:
        report = b.check_config(self.config)
        self.assertEqual(report["required"]["CONFIG_BLK_DEV_INITRD"], "y")
        self.config.write_text(GOOD_CONFIG.replace("CONFIG_BINFMT_ELF=y\n", ""))
        with self.assertRaisesRegex(BuildError, "CONFIG_BINFMT_ELF"):
            b.check_config(self.config)
        self.config.write_text(GOOD_CONFIG.replace("# CONFIG_INITRAMFS_FORCE is not set", "CONFIG_INITRAMFS_FORCE=y"))
        with self.assertRaisesRegex(BuildError, "INITRAMFS_FORCE"):
            b.check_config(self.config)


class NegativeTests(InitramfsTestBase):
    """Negative groups; every refusal checks the reason fragment, CLI runs also check exit code 1."""

    def test_expect_reject_checks_the_reason(self) -> None:
        ok = b._expect_reject("x", lambda: b.validate_nonce(""), "nonce is empty")
        self.assertEqual(ok["expected_fragment"], "nonce is empty")
        with self.assertRaisesRegex(BuildError, "another reason"):
            b._expect_reject("x", lambda: b.validate_nonce(""), "too short")
        with self.assertRaisesRegex(BuildError, "was accepted"):
            b._expect_reject("x", lambda: None, "anything")

    def test_evidence_path_outside_evidence_dir_is_rejected(self) -> None:
        never = b.WORK_ROOT / "initramfs-test-never"
        rc, err = run_main(["--evidence", str(self.tmp / "e.json"), "--output-dir", str(never)])
        self.assertEqual(rc, 1)
        self.assertIn("evidence path must be inside", err)
        self.assertFalse((self.tmp / "e.json").exists())
        self.assertFalse(never.exists())
        with self.assertRaisesRegex(BuildError, "evidence path"):
            b.evidence_file(str(b.EVIDENCE_DIR / ".." / "escape.json"))
        self.assertEqual(b.evidence_file(str(b.DEFAULT_EVIDENCE)), b.DEFAULT_EVIDENCE.resolve())

    def test_neg1_missing_init(self) -> None:
        archive = self.tmp / "empty.cpio"
        archive.write_bytes(b.pack_cpio([]))
        with self.assertRaisesRegex(BuildError, "no /init entry"):
            b.verify_archive(archive.read_bytes())
        rc, err = run_main(["--verify-archive", str(archive)])
        self.assertEqual(rc, 1)
        self.assertIn("no /init entry", err)

    def test_neg2_non_executable_init(self) -> None:
        archive = self.tmp / "noexec.cpio"
        archive.write_bytes(b.pack_cpio([("init", 0o100644, self.elf)]))
        with self.assertRaisesRegex(BuildError, "not executable"):
            b.verify_archive(archive.read_bytes())
        rc, err = run_main(["--verify-archive", str(archive)])
        self.assertEqual(rc, 1)
        self.assertIn("not executable", err)

    def test_neg3_invalid_nonce(self) -> None:
        cases = {
            "": "empty",
            "short": "too short",
            "under_score_nonce1": "[A-Za-z0-9]",  # 18 characters but not alphanumeric
            "abcd efgh ijkl mnop": "[A-Za-z0-9]",
            "abcdefgh-ijklmnop!": "[A-Za-z0-9]",
            "a" * 129: "too long",
        }
        for nonce, fragment in cases.items():
            with self.subTest(nonce=nonce):
                with self.assertRaises(BuildError) as ctx:
                    b.validate_nonce(nonce)
                self.assertIn(fragment, str(ctx.exception))
                never_dir = b.WORK_ROOT / "initramfs-test-never"
                never_evidence = b.EVIDENCE_DIR / "s4-36-test-never.json"
                rc, err = run_main(
                    ["--nonce", nonce, "--output-dir", str(never_dir), "--evidence", str(never_evidence)]
                )
                self.assertEqual(rc, 1)
                self.assertIn("error: nonce", err)
                self.assertFalse(never_dir.exists(), "no output directory may be created")
                self.assertFalse(never_evidence.exists(), "no evidence may be written")
        self.assertEqual(b.validate_nonce("AbCd1234EfGh5678"), "AbCd1234EfGh5678")

    def test_neg3b_nonce_baked_into_archive_is_rejected(self) -> None:
        nonce = "AbCd1234EfGh5678"
        archive = b.pack_cpio([("init", b.INIT_MODE, self.elf + nonce.encode())])
        with self.assertRaisesRegex(BuildError, "baked"):
            b.verify_archive(archive, nonce)
        archive = b.pack_cpio([("init", b.INIT_MODE, self.elf + b"od.nonce=0123456789abcdef")])
        with self.assertRaisesRegex(BuildError, "baked"):
            b.verify_archive(archive)
        self.assertRefused(self.elf + b"OD_NONCE=0123456789abcdef", "baked")

    def test_neg4_host_path_in_binary(self) -> None:
        for marker in (b"/Users/someone/work", b"/home/x", b"/private/var", b"/opt/tool"):
            with self.subTest(marker=marker):
                self.assertRefused(self.elf + marker + b"\0", "host path")
        archive = self.tmp / "host.cpio"
        archive.write_bytes(b.pack_cpio([("init", b.INIT_MODE, self.elf + b"/Users/someone\0")]))
        rc, err = run_main(["--verify-archive", str(archive)])
        self.assertEqual(rc, 1)
        self.assertIn("host path", err)

    def test_neg5_not_elf_and_wrong_machine(self) -> None:
        self.assertRefused(b"#!/bin/sh\nexit 0\n" * 8, "not an ELF")
        patched = bytearray(self.elf)
        struct.pack_into("<H", patched, 18, 183)  # EM_AARCH64
        self.assertRefused(bytes(patched), "e_machine 183")
        patched = bytearray(self.elf)
        struct.pack_into("<H", patched, 16, 3)  # ET_DYN
        self.assertRefused(bytes(patched), "e_type 3")

    @unittest.skipUnless(b.DEFAULT_CONFIG.is_file(), "staged kernel config is missing")
    def test_neg5b_not_elf_through_cli(self) -> None:
        fake = self.tmp / "fake-init"
        fake.write_bytes(b"not an elf at all" * 10)
        rc, err = run_main(
            ["--init-binary", str(fake), "--output-dir", str(b.WORK_ROOT / "initramfs-test-never"), "--no-evidence"]
        )
        self.assertEqual(rc, 1)
        self.assertIn("not an ELF", err)
        self.assertFalse((b.WORK_ROOT / "initramfs-test-never").exists())

    def test_extra_entries_and_trailing_garbage_are_rejected(self) -> None:
        archive = b.pack_cpio([("init", b.INIT_MODE, self.elf), ("etc", 0o040755, b"")])
        with self.assertRaisesRegex(BuildError, "unexpected entries"):
            b.verify_archive(archive)
        good = b.build_archive(self.elf)
        with self.assertRaisesRegex(BuildError, "after TRAILER"):
            b.verify_archive(good + b"junk")
        with self.assertRaisesRegex(BuildError, "truncated"):
            b.verify_archive(good[:-8])
        with self.assertRaisesRegex(BuildError, "bad magic"):
            b.verify_archive(b"X" * 200)

    def _code_segment(self) -> tuple[int, int, int]:
        """Return (phdr offset, file offset, filesz) of the executable PT_LOAD of the fixture."""
        phoff = struct.unpack_from("<Q", self.elf, 32)[0]
        phnum = struct.unpack_from("<H", self.elf, 56)[0]
        for index in range(phnum):
            at = phoff + 56 * index
            p_type, flags, offset, _v, _p, filesz, _m, _a = struct.unpack_from("<IIQQQQQQ", self.elf, at)
            if p_type == b.PT_LOAD and flags & b.PF_X:
                return at, offset, filesz
        raise AssertionError("fixture has no executable PT_LOAD")

    def _first_syscall(self) -> int:
        _at, offset, filesz = self._code_segment()
        position = self.elf[offset : offset + filesz].find(b"\x0f\x05")
        self.assertGreaterEqual(position, 5)
        return offset + position

    def test_writable_and_executable_segment_is_rejected(self) -> None:
        at, _offset, _filesz = self._code_segment()
        patched = bytearray(self.elf)
        flags = struct.unpack_from("<I", patched, at + 4)[0]
        struct.pack_into("<I", patched, at + 4, flags | b.PF_W)
        self.assertRefused(bytes(patched), "writable and executable")

    def test_int80_is_rejected(self) -> None:
        patched = bytearray(self.elf)
        position = self._first_syscall()
        patched[position : position + 2] = b"\xcd\x80"
        self.assertRefused(bytes(patched), "int 0x80")

    def test_syscall_number_outside_the_allowed_set_is_rejected(self) -> None:
        patched = bytearray(self.elf)
        position = self._first_syscall()
        struct.pack_into("<I", patched, position - 4, 231)  # exit_group is not in the allowed set
        self.assertRefused(bytes(patched), "syscall number 231")

    def test_cpio_header_fields_and_mode_are_checked(self) -> None:
        good = b.build_archive(self.elf)
        # newc header: magic 6 bytes, then 13 fields of 8 hex digits: ino mode uid gid nlink mtime ...
        for field, index in (("uid", 2), ("gid", 3), ("nlink", 4), ("mtime", 5)):
            with self.subTest(field=field):
                patched = bytearray(good)
                value = b"00000002" if field == "nlink" else b"00000001"
                patched[6 + 8 * index : 14 + 8 * index] = value
                with self.assertRaisesRegex(BuildError, "uid 0, gid 0, mtime 0, nlink 1"):
                    b.verify_archive(bytes(patched))
        with self.assertRaisesRegex(BuildError, "mode 100777, expected 100755"):
            b.verify_archive(b.pack_cpio([("init", 0o100777, self.elf)]))

    def test_duplicate_cpio_entry_is_rejected(self) -> None:
        archive = b.pack_cpio([("init", b.INIT_MODE, self.elf), ("init", b.INIT_MODE, self.elf)])
        with self.assertRaisesRegex(BuildError, "duplicate entry"):
            b.verify_archive(archive)

    def test_dynamic_or_interp_is_rejected(self) -> None:
        patched = bytearray(self.elf)
        phoff = struct.unpack_from("<Q", patched, 32)[0]
        struct.pack_into("<I", patched, phoff, b.PT_INTERP)  # first program header becomes PT_INTERP
        self.assertRefused(bytes(patched), "PT_INTERP")


if __name__ == "__main__":
    unittest.main(verbosity=2)
