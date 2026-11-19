#!/usr/bin/env python3
"""Tests of tools/build/build_firmware.py: image header, PE32+ checks, size checks, missing-tool messages.

Run from the repository root: `python3 -m unittest tools/build/test_build_firmware.py`.
"""

from __future__ import annotations

import hashlib
import importlib.util
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/build_firmware.py"
SPEC = importlib.util.spec_from_file_location("firmware_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)

BuildError = BUILDER.BuildError


def sample_raw(size: int = 0x100) -> bytes:
    return bytes(BUILDER.HEADER_SIZE) + bytes((index * 7 + 1) & 0xFF for index in range(size - BUILDER.HEADER_SIZE))


def toolchain_available() -> bool:
    try:
        BUILDER.resolve_tools()
    except BuildError:
        return False
    return True


class ImageHeaderTest(unittest.TestCase):
    def test_crc_check_value(self) -> None:
        self.assertEqual(BUILDER.crc32_ieee(b"123456789"), 0xCBF4_3926)

    def test_header_fields_and_layout(self) -> None:
        image = BUILDER.assemble_image(sample_raw(), 0x40)
        self.assertEqual(len(image), 0x0040_0000)
        self.assertEqual(image[:8], b"ODFWIMG1")
        self.assertEqual(struct.unpack_from("<Q", image, 0)[0], 0x3147_4D49_5746_444F)
        self.assertEqual(struct.unpack_from("<II", image, 0x08), (1, 0x40))
        self.assertEqual(struct.unpack_from("<QQQ", image, 0x10), (0x0040_0000, 0x0020_0000, 0x40))
        self.assertEqual(struct.unpack_from("<HHI", image, 0x28), (1, 11, 0))
        self.assertEqual(image[0x34:0x40], bytes(12))
        # code after the header is untouched, the tail is zero padding
        self.assertEqual(image[0x40:0x100], sample_raw()[0x40:])
        self.assertEqual(image[0x100:], bytes(0x0040_0000 - 0x100))

    def test_crc_covers_whole_image_with_field_zeroed(self) -> None:
        image = BUILDER.assemble_image(sample_raw(), 0x40)
        stored = struct.unpack_from("<I", image, 0x30)[0]
        zeroed = image[:0x30] + bytes(4) + image[0x34:]
        self.assertEqual(stored, BUILDER.crc32_ieee(zeroed))
        self.assertNotEqual(stored, 0)

    def test_validate_accepts_assembled_image_and_reports_fields(self) -> None:
        fields = BUILDER.validate_image(BUILDER.assemble_image(sample_raw(), 0x48))
        self.assertEqual(fields["entry_offset"], 0x48)
        self.assertEqual(fields["link_base"], 0x0020_0000)

    def test_validate_rejects_wrong_sizes(self) -> None:
        image = BUILDER.assemble_image(sample_raw(), 0x40)
        for bad in (image[:-1], image + b"\0", b""):
            with self.assertRaisesRegex(BuildError, "expected exactly 0x400000"):
                BUILDER.validate_image(bad)

    def test_validate_rejects_corruption(self) -> None:
        image = bytearray(BUILDER.assemble_image(sample_raw(), 0x40))
        magic = bytearray(image)
        magic[0] ^= 0xFF
        with self.assertRaisesRegex(BuildError, "magic"):
            BUILDER.validate_image(bytes(magic))
        flipped = bytearray(image)
        flipped[0x200000] ^= 1  # a padding byte: only the CRC can see it
        with self.assertRaisesRegex(BuildError, "image_crc32"):
            BUILDER.validate_image(bytes(flipped))
        reserved = bytearray(image)
        reserved[0x3F] = 1
        with self.assertRaisesRegex(BuildError, "reserved"):
            BUILDER.validate_image(bytes(reserved))
        flags = bytearray(image)
        flags[0x2C] = 1
        with self.assertRaisesRegex(BuildError, "flags"):
            BUILDER.validate_image(bytes(flags))

    def test_assemble_rejects_oversize_and_bad_raw(self) -> None:
        with self.assertRaisesRegex(BuildError, "larger than the fixed image size"):
            BUILDER.assemble_image(sample_raw(0x0040_0001), 0x40)
        with self.assertRaisesRegex(BuildError, "no code after"):
            BUILDER.assemble_image(bytes(0x40), 0x40)
        dirty = bytearray(sample_raw())
        dirty[0] = 1
        with self.assertRaisesRegex(BuildError, "header area"):
            BUILDER.assemble_image(bytes(dirty), 0x40)
        for entry in (0x3F, 0x0040_0000):
            with self.assertRaisesRegex(BuildError, "entry offset"):
                BUILDER.assemble_image(sample_raw(), entry)


# Shared with firmware/uefi/src/image.rs (`REFERENCE_HEADER`, `SAMPLE_IMAGE_CRC32`): the Rust and the
# Python validators are pinned to the same bytes. entry_offset = 0x40, image_crc32 = 0x12345678.
REFERENCE_HEADER_HEX = (
    "4f444657494d4731" "01000000" "40000000" "0000400000000000" "0000200000000000" "4000000000000000"
    "0100" "0700" "00000000" "78563412" "000000000000000000000000"
)
SAMPLE_IMAGE_CRC32 = 0x9971_494B


class CrossValidatorTest(unittest.TestCase):
    def test_header_matches_reference_bytes_checked_by_rust(self) -> None:
        header = BUILDER.build_header(0x40, 0x1234_5678, handoff_minor_max=7)
        self.assertEqual(header.hex(), REFERENCE_HEADER_HEX)
        self.assertEqual(len(header), 0x40)
        self.assertEqual(struct.unpack_from("<I", header, 0x30)[0], 0x1234_5678)
        self.assertEqual(header[0x34:0x40], bytes(12))

    def test_sample_image_crc_matches_value_checked_by_rust(self) -> None:
        raw = bytes(0x40) + bytes([0x90, 0x90, 0x90, 0x90, 0xF4, 0xEB, 0xFD, 0x00])
        image = BUILDER.assemble_image(raw, 0x40, handoff_minor_max=7)
        self.assertEqual(BUILDER.image_crc(image), SAMPLE_IMAGE_CRC32)
        self.assertEqual(struct.unpack_from("<I", image, 0x30)[0], SAMPLE_IMAGE_CRC32)
        self.assertEqual(BUILDER.validate_image(image)["image_crc32"], SAMPLE_IMAGE_CRC32)


class FirmwareVolumeTest(unittest.TestCase):
    def test_archive_matches_wire_bytes_and_ignores_input_order(self) -> None:
        ordered = BUILDER.pack_fv({"a": b"!", "b.efi": b"\x00\xff"})
        self.assertEqual(ordered, BUILDER.pack_fv({"b.efi": b"\x00\xff", "a": b"!"}))
        records = b"\x01\x00\x00\x00\x01\x00\x00\x00a!\x05\x00\x00\x00\x02\x00\x00\x00b.efi\x00\xff"
        header = b"ODFVARC\0\x01\x00\x18\x00\x31\x00\x00\x00\x02\x00\x00\x00"
        checksum = zlib.crc32(header + bytes(4) + records)
        self.assertEqual(ordered, header + checksum.to_bytes(4, "little") + records)

    def test_invalid_name_empty_file_and_oversize_fail(self) -> None:
        for name in ("", ".", "..", "A", "app/path", "app\0", "ё", "a" * 64):
            with self.subTest(name=name), self.assertRaisesRegex(BuildError, "invalid FV name"):
                BUILDER.pack_fv({name: b"x"})
        with self.assertRaisesRegex(BuildError, "empty FV file"):
            BUILDER.pack_fv({"hello": b""})
        with self.assertRaisesRegex(BuildError, "ROM capacity"):
            BUILDER.pack_fv({"hello": bytes(BUILDER.IMAGE_SIZE)})

    def test_with_test_apps_is_available_only_for_a_rom_build(self) -> None:
        self.assertTrue(BUILDER.parse_args(["--with-test-apps"]).with_test_apps)
        for other in (["--smoke"], ["--efi-probe", "probe.efi"]):
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--with-test-apps", *other],
                capture_output=True, text=True, cwd=ROOT, check=False,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("requires a ROM build", result.stderr)


class SmokeImageTest(unittest.TestCase):
    def test_smoke_image_is_a_valid_v1_image_with_the_expected_code(self) -> None:
        image = BUILDER.build_smoke_image()
        self.assertEqual(len(image), 0x0040_0000)
        fields = BUILDER.validate_image(image)
        self.assertEqual(fields["handoff_minor_max"], 4)
        self.assertEqual(fields["entry_offset"], 0x40)
        # mov al,'X' ; mov dx,0x3F8 ; out dx,al ; hlt
        self.assertEqual(image[0x40:0x48], bytes.fromhex("b058" "66baf803" "ee" "f4"))
        self.assertEqual(image[0x48:], bytes(0x0040_0000 - 0x48))

    def test_smoke_flag_writes_the_image_without_a_toolchain(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "nested" / "mini.bin"
            env = dict(os.environ, PATH="/nonexistent")
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--smoke", "--out", str(out)],
                capture_output=True,
                text=True,
                env=env,
                cwd=tmp,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            data = out.read_bytes()
            self.assertEqual(data, BUILDER.build_smoke_image())
            self.assertIn(hashlib.sha256(data).hexdigest(), result.stdout)

    def test_smoke_default_out_is_mini_bin_never_the_firmware_image(self) -> None:
        smoke = BUILDER.parse_args(["--smoke"])
        self.assertEqual(smoke.out, BUILDER.ROOT / "work/stage7/mini.bin")
        self.assertNotEqual(smoke.out, BUILDER.DEFAULT_OUT)
        self.assertEqual(BUILDER.parse_args([]).out, BUILDER.ROOT / "work/stage7/firmware.bin")
        self.assertEqual(BUILDER.parse_args(["--smoke", "--out", "x.bin"]).out, Path("x.bin"))

    def test_smoke_and_efi_probe_are_mutually_exclusive(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--smoke", "--efi-probe", "x.efi"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("mutually exclusive", result.stderr)


class MissingToolsTest(unittest.TestCase):
    def test_missing_clang_has_clear_message(self) -> None:
        env = {"PATH": "", "OD_FW_CLANG": "/nonexistent/clang"}
        with self.assertRaises(BuildError) as context:
            BUILDER.resolve_tools(env)
        message = str(context.exception)
        self.assertIn("OD_FW_CLANG=/nonexistent/clang", message)
        self.assertIn("clang", message)

    def test_missing_clang_on_empty_path(self) -> None:
        with self.assertRaisesRegex(BuildError, "could not find 'clang' on PATH"):
            BUILDER.resolve_tools({"PATH": ""})

    def test_missing_rust_objcopy_has_clear_message(self) -> None:
        if not toolchain_available():
            self.skipTest("the firmware toolchain is not installed")
        env = dict(os.environ)
        env["OD_FW_RUST_OBJCOPY"] = "/nonexistent/rust-objcopy"
        with self.assertRaisesRegex(BuildError, "OD_FW_RUST_OBJCOPY=/nonexistent/rust-objcopy"):
            BUILDER.resolve_tools(env)

    def test_missing_rust_target_has_install_hint(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fake_rustc = Path(directory) / "rustc"
            fake_rustc.write_text(
                "#!/bin/sh\n"
                'if [ "$1" = "-vV" ]; then echo "host: aarch64-apple-darwin"; exit 0; fi\n'
                f'if [ "$1" = "--print" ]; then echo "{directory}/sysroot"; exit 0; fi\n'
                "exit 1\n"
            )
            fake_rustc.chmod(0o755)
            clang = shutil.which("clang") or "/usr/bin/true"
            cargo = shutil.which("cargo") or "/usr/bin/true"
            env = {"PATH": "", "OD_FW_CLANG": clang, "OD_FW_CARGO": cargo, "OD_FW_RUSTC": str(fake_rustc)}
            with self.assertRaisesRegex(BuildError, "rustup target add x86_64-unknown-none"):
                BUILDER.resolve_tools(env)


class ElfCheckTest(unittest.TestCase):
    @staticmethod
    def elf(e_type: int = 2, vaddr: int = 0x0020_0000, entry: int = 0x0020_0040, filesz: int = 0x100) -> bytes:
        header = bytearray(64)
        header[:4] = b"\x7fELF"
        header[4], header[5] = 2, 1
        struct.pack_into("<H", header, 16, e_type)
        struct.pack_into("<Q", header, 24, entry)
        struct.pack_into("<Q", header, 32, 64)
        struct.pack_into("<HH", header, 54, 56, 1)
        phdr = struct.pack("<IIQQQQQQ", 1, 7, 0x1000, vaddr, vaddr, filesz, filesz, 0x1000)
        return bytes(header) + phdr + bytes(0x1000 - 64 - 56) + bytes(filesz)

    def test_accepts_static_flat_elf(self) -> None:
        self.assertEqual(BUILDER.inspect_elf(self.elf()), (0x0020_0040, 0x1000, 0x100))

    def test_rejects_pie_wrong_base_and_entry(self) -> None:
        with self.assertRaisesRegex(BuildError, "ET_EXEC"):
            BUILDER.inspect_elf(self.elf(e_type=3))
        with self.assertRaisesRegex(BuildError, "link base"):
            BUILDER.inspect_elf(self.elf(vaddr=0x0030_0000))
        with self.assertRaisesRegex(BuildError, "entry"):
            BUILDER.inspect_elf(self.elf(entry=0x0020_0010))


class PeCheckTest(unittest.TestCase):
    @staticmethod
    def pe(
        machine: int = 0x8664,
        magic: int = 0x20B,
        subsystem: int = 10,
        section: bytes = b".reloc",
        reloc_rva: int = 0x4000,
        reloc_size: int = 12,
    ) -> bytes:
        """A synthetic PE32+ header: one section, 16 data directories, DataDirectory[5] = base relocs."""
        pe_offset = 0x40
        optional_size = 112 + 16 * 8
        image = bytearray(0x400)
        image[:2] = b"MZ"
        struct.pack_into("<I", image, 0x3C, pe_offset)
        image[pe_offset : pe_offset + 4] = b"PE\0\0"
        struct.pack_into("<HHIIIHH", image, pe_offset + 4, machine, 1, 0, 0, 0, optional_size, 0x22)
        optional = pe_offset + 24
        struct.pack_into("<H", image, optional, magic)
        struct.pack_into("<H", image, optional + 68, subsystem)
        struct.pack_into("<I", image, optional + 108, 16)
        struct.pack_into("<II", image, optional + 112 + 8 * 5, reloc_rva, reloc_size)
        table = optional + optional_size
        struct.pack_into("<8sIII", image, table, section.ljust(8, b"\0"), 12, 0x4000, 0x200)
        return bytes(image)

    def test_accepts_x64_efi_application_with_reloc(self) -> None:
        fields = BUILDER.inspect_pe(self.pe())
        self.assertEqual(fields["machine"], 0x8664)
        self.assertEqual(fields["magic"], 0x20B)
        self.assertEqual(fields["subsystem"], 10)
        self.assertEqual(fields["reloc_rva"], 0x4000)

    def test_rejects_wrong_machine_magic_and_subsystem(self) -> None:
        with self.assertRaisesRegex(BuildError, "Machine"):
            BUILDER.inspect_pe(self.pe(machine=0xAA64))
        with self.assertRaisesRegex(BuildError, "magic"):
            BUILDER.inspect_pe(self.pe(magic=0x10B))
        for subsystem in (3, 11, 12):
            with self.assertRaisesRegex(BuildError, "Subsystem"):
                BUILDER.inspect_pe(self.pe(subsystem=subsystem))

    def test_rejects_missing_reloc(self) -> None:
        with self.assertRaisesRegex(BuildError, "no .reloc section"):
            BUILDER.inspect_pe(self.pe(section=b".text"))
        with self.assertRaisesRegex(BuildError, "empty"):
            BUILDER.inspect_pe(self.pe(reloc_rva=0, reloc_size=0))
        with self.assertRaisesRegex(BuildError, "inside the .reloc section"):
            BUILDER.inspect_pe(self.pe(reloc_rva=0x9000))

    def test_rejects_garbage(self) -> None:
        with self.assertRaisesRegex(BuildError, "MZ"):
            BUILDER.inspect_pe(b"\x7fELF" + bytes(100))
        broken = bytearray(self.pe())
        broken[0x40] = 0
        with self.assertRaisesRegex(BuildError, "PE signature"):
            BUILDER.inspect_pe(bytes(broken))


class CommandLineTest(unittest.TestCase):
    def test_acpi_mode_refuses_to_overwrite_a_hardlinked_handoff(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            input_path = Path(directory) / "handoff.bin"
            output_path = Path(directory) / "output.bin"
            original = b"handoff sentinel"
            input_path.write_bytes(original)
            os.link(input_path, output_path)
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--acpi-blob", input_path, "--out", output_path],
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("ACPI output and handoff input must be distinct", result.stderr)
            self.assertEqual(input_path.read_bytes(), original)

    def test_missing_tool_exits_nonzero_with_message(self) -> None:
        env = dict(os.environ)
        env["OD_FW_CLANG"] = "/nonexistent/clang"
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--out", str(Path(directory) / "fw.bin")],
                capture_output=True,
                text=True,
                env=env,
                check=False,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("OD_FW_CLANG=/nonexistent/clang", result.stderr)
            self.assertFalse((Path(directory) / "fw.bin").exists())


def probe_toolchain_available() -> bool:
    try:
        BUILDER.resolve_probe_tools()
    except BuildError:
        return False
    return True


@unittest.skipUnless(toolchain_available() and probe_toolchain_available(), "requires both firmware targets")
class FullBuildTest(unittest.TestCase):
    def test_build_produces_valid_image(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "firmware.bin"
            target_dir = Path(directory) / "target"
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--target-dir", str(target_dir), "--out", str(out)],
                capture_output=True,
                text=True,
                cwd=ROOT,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            image = out.read_bytes()
            self.assertEqual(len(image), 0x0040_0000)
            fields = BUILDER.validate_image(image)
            self.assertGreaterEqual(fields["entry_offset"], 0x40)
            self.assertIn(f"sha256: {hashlib.sha256(image).hexdigest()}", result.stdout)
            self.assertIn("size: 0x400000", result.stdout)
            # the entry stub starts with `cld` (0xFC), see firmware/uefi/src/start.S
            self.assertEqual(image[fields["entry_offset"]], 0xFC)
            driver = (target_dir / "runtime" / BUILDER.PROBE_TARGET / "release" / "od-runtime.efi").read_bytes()
            self.assertEqual(BUILDER.inspect_pe(driver, 12)["subsystem"], 12)
            self.assertIn(BUILDER.pack_fv({"runtime": driver}), image)


@unittest.skipUnless(probe_toolchain_available(), "the x86_64-unknown-uefi target is not installed")
class EfiProbeBuildTest(unittest.TestCase):
    def test_probe_builds_valid_pe32_plus(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "efi_probe.efi"
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--efi-probe", str(out)],
                capture_output=True,
                text=True,
                cwd=ROOT,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            pe = out.read_bytes()
            fields = BUILDER.inspect_pe(pe)
            self.assertEqual(fields["subsystem"], 10)
            self.assertIn(f"sha256: {hashlib.sha256(pe).hexdigest()}", result.stdout)
            self.assertIn("Subsystem=10", result.stdout)


@unittest.skipUnless(toolchain_available() and probe_toolchain_available(), "requires both firmware targets")
class FirmwareVolumeBuildTest(unittest.TestCase):
    def test_optional_test_apps_are_reproducible_pe_images_linked_into_the_rom(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            target_dir = Path(directory) / "target"
            out = Path(directory) / "firmware-with-apps.bin"
            for build_dir in (target_dir, target_dir.with_name("second-target")):
                result = subprocess.run(
                    [sys.executable, str(SCRIPT), "--with-test-apps", "--target-dir", str(build_dir), "--out", str(out)],
                    capture_output=True, text=True, cwd=ROOT, check=False,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                if build_dir == target_dir:
                    first_sha256 = hashlib.sha256(out.read_bytes()).hexdigest()
                else:
                    self.assertEqual(hashlib.sha256(out.read_bytes()).hexdigest(), first_sha256)
            app_dir = target_dir / "test-apps" / BUILDER.PROBE_TARGET / "release"
            files = {name: (app_dir / f"{name}.efi").read_bytes() for name in BUILDER.TEST_APPS}
            for name, pe in files.items():
                with self.subTest(name=name):
                    self.assertEqual(BUILDER.inspect_pe(pe)["subsystem"], 10)
            driver = target_dir / "runtime" / BUILDER.PROBE_TARGET / "release" / "od-runtime.efi"
            archive = BUILDER.pack_fv({"runtime": driver.read_bytes(), **files})
            actual = target_dir / "fv" / hashlib.sha256(archive).hexdigest() / "archive.bin"
            self.assertEqual(actual.read_bytes(), archive)
            image = out.read_bytes()
            BUILDER.validate_image(image)
            self.assertEqual(image.count(archive), 1)


if __name__ == "__main__":
    unittest.main()
