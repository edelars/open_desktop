#!/usr/bin/env python3
"""Paired deterministic build test for the static Stage 5 E input."""

from __future__ import annotations

import hashlib
import re
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
WORK = ROOT / "work/stage5"
sys.path.insert(0, str(ROOT))

from tools.build import build_stage5_rootfs as builder

ACCEPTED_ROOTFS_SHA256 = "c2b2c94665ea256c6fa19c7eeebfc8451589028df3da00b2e1e05c7473dd2895"
STAGE5_E_SOURCE_SHA256 = "9341d7fa286e8355718483e28f05d001ea37275a9e610d5fdbe1944faf08f700"
STAGE5_E_STATIC_SHA256 = "da18de76182af4f986a076c8ec9796ae0db2af365104788929e22643143d8024"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def assert_amd64_elf(path: Path) -> None:
    header = path.read_bytes()[:20]
    if len(header) != 20 or header[:4] != b"\x7fELF":
        raise AssertionError("exported E artifact is not an ELF file")
    if header[4:6] != b"\x02\x01":
        raise AssertionError("exported E artifact must be ELF64 little-endian")
    machine = struct.unpack_from("<H", header, 18)[0]
    if machine != 62:
        raise AssertionError(f"exported E ELF machine must be x86-64 (62), got {machine}")


class Stage5EStaticBuildTest(unittest.TestCase):
    def test_guest_c_scenarios_use_compiler_path_present_in_accepted_rootfs(self) -> None:
        scripts = ROOT / "guest/linux/tests"
        for letter, name in (
            ("D", "s5_d_compile.sh"),
            ("E", "s5_e_wx_interpreter.sh"),
            ("F", "s5_f_network_transfer.sh"),
            ("G", "s5_g_bounded_memory_stop.sh"),
        ):
            with self.subTest(scenario=letter):
                source = (scripts / name).read_text()
                self.assertIn("/usr/bin/gcc-15 ", source)
                self.assertNotRegex(source, r"(?<![A-Za-z0-9_./-])gcc\s")

    def test_pair_has_identical_e_elf_and_accepted_rootfs_bytes(self) -> None:
        source = ROOT / "tests/fixtures/guest/stage5/stage5_e.c"
        self.assertEqual(sha256(source), STAGE5_E_SOURCE_SHA256)
        WORK.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="s5-e-static-pair-", dir=WORK) as temporary:
            output_dir = Path(temporary)
            first_rootfs = output_dir / "first.raw"
            first_elf = output_dir / "first-stage5-e.elf"
            second_rootfs = output_dir / "second.raw"
            second_elf = output_dir / "second-stage5-e.elf"

            first_rootfs_hash = builder.build(first_rootfs, first_elf)
            second_rootfs_hash = builder.build(second_rootfs, second_elf)
            first_elf_hash = sha256(first_elf)
            second_elf_hash = sha256(second_elf)
            assert_amd64_elf(first_elf)
            assert_amd64_elf(second_elf)

        self.assertEqual(first_rootfs_hash, ACCEPTED_ROOTFS_SHA256)
        self.assertEqual(second_rootfs_hash, ACCEPTED_ROOTFS_SHA256)
        self.assertEqual(first_elf_hash, second_elf_hash)
        self.assertEqual(first_elf_hash, STAGE5_E_STATIC_SHA256)
        print(f"stage5_e_source_sha256={STAGE5_E_SOURCE_SHA256}")
        print(f"stage5_e_static_elf_sha256={first_elf_hash}")
        print(f"stage5_rootfs_sha256={first_rootfs_hash}")
        print("stage5_e_static_elf_executed=false")

    def test_raced_e_output_is_never_removed_by_failed_exclusive_create(self) -> None:
        builder.WORK.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="s5-e-output-race-", dir=builder.WORK) as temporary:
            output_dir = Path(temporary)
            rootfs = output_dir / "race.raw"
            e_output = output_dir / "stage5-e-static.elf"

            def prepare_inputs_with_racer() -> None:
                e_output.write_bytes(b"created by competing producer")

            def fake_run(command: list[str]) -> str:
                if command[0] == "cargo":
                    target = Path(command[-3]) / command[-2]
                    target.write_bytes(b"new raw output")
                    return ""
                raise AssertionError("Docker must not run after exclusive create loses the race")

            with patch.object(builder, "prepare_inputs", side_effect=prepare_inputs_with_racer), \
                 patch.object(builder, "run", side_effect=fake_run), \
                 patch.object(builder.shutil, "which", return_value="docker"):
                with self.assertRaises(FileExistsError):
                    builder.build(rootfs, e_output)

            self.assertEqual(e_output.read_bytes(), b"created by competing producer")
            self.assertFalse(rootfs.exists())
            self.assertFalse((output_dir / "race.raw.package-files.txt").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
