#!/usr/bin/env python3
"""Tests of the S4-38 L2 oracle tool and the static ISA inventory.

All tests use small synthetic files in temporary directories. The real ~1.3 GB kernel files are only
used by `RealArtifactTest`, which is skipped unless `OD_S4_38L2_REAL=1` is set and the staged
directory exists (it takes minutes and reads 500 MB).
"""

from __future__ import annotations

import importlib.util
import os
import shutil
import struct
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/build"))


def load(name: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / f"tools/build/{name}.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


ORACLE = load("s4_38l2_oracle")
INVENTORY = load("s4_38l2_isa_inventory")
STATUS = load("s4_38l2_contract_status")

ZSTD = ORACLE.DEFAULT_ZSTD if Path(ORACLE.DEFAULT_ZSTD).is_file() else shutil.which("zstd")
OBJDUMP = INVENTORY.DEFAULT_OBJDUMP if Path(INVENTORY.DEFAULT_OBJDUMP).is_file() else None
BUILD_ID = bytes(range(0x10, 0x24))


def build_elf(build_id: bytes = BUILD_ID, load0: bytes = b"\x90" * 64, load1: bytes = b"\xcc" * 32) -> bytes:
    """Tiny ELF64: two PT_LOAD + PT_NOTE (GNU build-id) + SYMTAB with `startup_64` and `verify_cpu`."""
    note = struct.pack("<III", 4, len(build_id), 3) + b"GNU\x00" + build_id
    strtab = b"\x00startup_64\x00verify_cpu\x00"
    shstrtab = b"\x00.symtab\x00.strtab\x00.shstrtab\x00"

    def sym(name: int, value: int, size: int) -> bytes:
        return struct.pack("<IBBHQQ", name, 0x12, 0, 1, value, size)

    symtab = bytes(24) + sym(1, 0xFFFFFFFF81000000, 8) + sym(12, 0xFFFFFFFF81000010, 32)
    ph_off, ph_size = 64, 56 * 3
    off_load0 = 0x200
    off_load1 = off_load0 + len(load0)
    off_note = off_load1 + len(load1)
    off_sym = off_note + len(note)
    off_str = off_sym + len(symtab)
    off_shstr = off_str + len(strtab)
    off_sh = (off_shstr + len(shstrtab) + 7) & ~7
    out = bytearray(off_sh + 64 * 4)
    ehdr = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8)
    ehdr += struct.pack("<HHIQQQIHHHHHH", 2, 62, 1, 0x1000000, ph_off, off_sh, 0, 64, 56, 3, 64, 4, 3)
    out[:64] = ehdr
    phdrs = struct.pack("<IIQQQQQQ", 1, 5, off_load0, 0xFFFFFFFF81000000, 0x1000000, len(load0), len(load0), 0x1000)
    phdrs += struct.pack("<IIQQQQQQ", 1, 6, off_load1, 0xFFFFFFFF81100000, 0x1100000, len(load1), len(load1) + 8, 0x1000)
    phdrs += struct.pack("<IIQQQQQQ", 4, 0, off_note, 0, 0, len(note), len(note), 4)
    out[ph_off : ph_off + ph_size] = phdrs
    out[off_load0 : off_load0 + len(load0)] = load0
    out[off_load1 : off_load1 + len(load1)] = load1
    out[off_note : off_note + len(note)] = note
    out[off_sym : off_sym + len(symtab)] = symtab
    out[off_str : off_str + len(strtab)] = strtab
    out[off_shstr : off_shstr + len(shstrtab)] = shstrtab
    sh = bytes(64)
    sh += struct.pack("<IIQQQQIIQQ", 1, 2, 0, 0, off_sym, len(symtab), 2, 1, 8, 24)
    sh += struct.pack("<IIQQQQIIQQ", 9, 3, 0, 0, off_str, len(strtab), 0, 0, 1, 0)
    sh += struct.pack("<IIQQQQIIQQ", 17, 3, 0, 0, off_shstr, len(shstrtab), 0, 0, 1, 0)
    out[off_sh : off_sh + len(sh)] = sh
    return bytes(out)


def zstd_compress(data: bytes) -> bytes:
    import subprocess

    return subprocess.run([ZSTD, "-q", "-c", "--check"], input=data, capture_output=True, check=True).stdout


def build_bzimage(vmlinux: bytes, *, setup_sects: int = 2, payload_offset: int = 0x40, trailer: int | None = None) -> bytes:
    stream = zstd_compress(vmlinux)
    payload = stream + struct.pack("<I", len(vmlinux) if trailer is None else trailer)
    pm = bytearray(payload_offset) + payload + b"tail-code" * 8
    header = bytearray((setup_sects + 1) * 512)
    header[0x1F1] = setup_sects
    struct.pack_into("<H", header, 0x1FE, 0xAA55)
    header[0x202:0x206] = b"HdrS"
    struct.pack_into("<H", header, 0x206, 0x20F)
    struct.pack_into("<I", header, 0x230, 0x200000)
    struct.pack_into("<II", header, 0x248, payload_offset, len(payload))
    struct.pack_into("<Q", header, 0x258, 0x1000000)
    struct.pack_into("<I", header, 0x260, 0x44C1000)
    return bytes(header) + bytes(pm)


@unittest.skipUnless(ZSTD, "host zstd is required")
class OracleTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def write(self, name: str, data: bytes) -> Path:
        path = self.tmp / name
        path.write_bytes(data)
        return path

    def test_header_payload_and_trailer(self) -> None:
        elf = build_elf()
        image = build_bzimage(elf)
        header = ORACLE.parse_setup_header(image)
        self.assertEqual(header["pm_offset"], 3 * 512)
        payload = ORACLE.extract_payload(image, header)
        stream, trailer = ORACLE.split_trailer(payload)
        self.assertEqual(trailer, len(elf))
        ok, diag = ORACLE.zstd_decompress(stream, ZSTD, self.tmp / "out")
        self.assertTrue(ok, diag)
        self.assertEqual((self.tmp / "out").read_bytes(), elf)

    def test_header_rejects_bad_magic_and_range(self) -> None:
        image = bytearray(build_bzimage(build_elf()))
        image[0x202:0x206] = b"XXXX"
        with self.assertRaises(ORACLE.OracleError):
            ORACLE.parse_setup_header(bytes(image))
        good = build_bzimage(build_elf())
        header = ORACLE.parse_setup_header(good)
        with self.assertRaises(ORACLE.OracleError):
            ORACLE.extract_payload(good[:-300], header)

    def test_trailer_left_in_stream_is_rejected_by_zstd(self) -> None:
        elf = build_elf()
        stream = zstd_compress(elf)
        ok, _ = ORACLE.zstd_decompress(stream + struct.pack("<I", len(elf)), ZSTD, self.tmp / "out")
        self.assertFalse(ok)
        self.assertFalse((self.tmp / "out").exists())

    def test_identical_elf_matches(self) -> None:
        elf = build_elf()
        a, b = ORACLE.Elf64(self.write("a", elf)), ORACLE.Elf64(self.write("b", elf))
        verdict = ORACLE.check_elf_against_reference(a, b)
        self.assertTrue(verdict["ok"])
        self.assertEqual(verdict["build_id_candidate"], BUILD_ID.hex())
        self.assertEqual([s["filesz"] for s in verdict["pt_load"]["segments"]], [64, 32])

    def test_injected_pt_load_byte_is_detected(self) -> None:
        elf = bytearray(build_elf())
        a = ORACLE.Elf64(self.write("a", bytes(elf)))
        offset = a.loads()[1]["offset"] + 5
        elf[offset] ^= 0x01
        b = ORACLE.Elf64(self.write("b", bytes(elf)))
        verdict = ORACLE.check_elf_against_reference(a, b)
        self.assertFalse(verdict["ok"])
        self.assertTrue(verdict["build_id_equal"])
        segment = verdict["pt_load"]["segments"][1]
        self.assertFalse(segment["equal"])
        self.assertEqual(segment["first_diff_offset"], 5)
        self.assertTrue(verdict["pt_load"]["segments"][0]["equal"])

    def test_foreign_build_id_is_detected(self) -> None:
        a = ORACLE.Elf64(self.write("a", build_elf()))
        b = ORACLE.Elf64(self.write("b", build_elf(build_id=bytes(range(0x50, 0x64)))))
        verdict = ORACLE.check_elf_against_reference(a, b)
        self.assertFalse(verdict["build_id_equal"])
        self.assertFalse(verdict["ok"])
        self.assertTrue(verdict["pt_load"]["equal"])  # only the id differs: build-id alone must reject

    def test_corrupted_payload_byte_fails_zstd_or_comparison(self) -> None:
        elf = build_elf(load0=bytes(range(256)) * 40)
        image = build_bzimage(elf)
        header = ORACLE.parse_setup_header(image)
        stream, trailer = ORACLE.split_trailer(ORACLE.extract_payload(image, header))
        reference = ORACLE.Elf64(self.write("ref", elf))
        candidate = ORACLE.Elf64(self.write("cand", elf))
        results = ORACLE.run_injections(image, header, stream, trailer, candidate, ZSTD, self.tmp, self.tmp / "ref")
        by_name = {r["name"]: r for r in results}
        self.assertTrue(by_name["corrupted_payload_byte"]["detected"])
        self.assertTrue(by_name["size_trailer_not_removed"]["detected"])
        self.assertTrue(by_name["foreign_vmlinux_build_id"]["detected"])
        self.assertFalse(by_name["foreign_vmlinux_build_id"]["build_id_equal"])
        del reference

    def test_symbols_and_checkpoint_addresses(self) -> None:
        elf = ORACLE.Elf64(self.write("a", build_elf()))
        syms = elf.symbols({"startup_64", "verify_cpu", "missing"})
        self.assertEqual(syms["startup_64"][0]["value"], 0xFFFFFFFF81000000)
        self.assertNotIn("missing", syms)
        self.assertEqual(elf.va_to_pa_and_offset(0xFFFFFFFF81000010), (0x1000010, 0x210))
        self.assertIsNone(elf.va_to_pa_and_offset(0xFFFFFFFF90000000))
        table = ORACLE.checkpoint_table(elf, 0x1000000)
        self.assertIn("symbol x86_64_start_kernel missing", table["problems"])
        by_name = {r["name"]: r for r in table["rows"]}
        self.assertEqual(by_name["startup_64"]["pa"], 0x1000000)
        self.assertEqual(by_name["cpuid1"]["va"], 0xFFFFFFFF81000010 + 0x87)

    def test_relocated_copy_is_derived_from_image_bytes(self) -> None:
        header = {"kernel_alignment": 0x200000, "pref_address": 0x1000000, "init_size": 0x44C1000}
        code = bytearray(0x300)
        entry = bytes.fromhex("8b9e60020000") + bytes.fromhex("81eb00b00e01") + bytes.fromhex("4801eb")
        code[0x200 : 0x200 + len(entry)] = entry
        lea = bytes.fromhex("488d83c0594001") + bytes.fromhex("ffe0")  # lea rax,[rbx+0x14059c0]; jmp rax
        code[0x240 : 0x240 + len(lea)] = lea
        info = ORACLE.relocated_copy_address(bytes(code), header, 0x1000000)
        self.assertEqual(info["rva_end_imm32"], 0x10EB000)
        self.assertEqual(info["rbx_relocated_copy"], 0x1000000 + 0x44C1000 - 0x10EB000)
        self.assertEqual(info["relocated_rip_first_fetch"], info["rbx_relocated_copy"] + 0x14059C0)
        with self.assertRaises(ORACLE.OracleError):
            ORACLE.relocated_copy_address(bytes(0x300), header, 0x1000000)

    def test_early_serial_decision_follows_cmdline_parser(self) -> None:
        base = "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=ab nokaslr"
        self.assertFalse(ORACLE.early_serial_decision(base)["early_serial_base_nonzero"])
        self.assertTrue(ORACLE.early_serial_decision(base + " earlyprintk=serial,ttyS0,115200")["early_serial_base_nonzero"])
        self.assertTrue(ORACLE.early_serial_decision("console=uart8250,io,0x3f8,115200n8")["early_serial_base_nonzero"])
        self.assertTrue(ORACLE.early_serial_decision("console=uart,io,0x3f8")["early_serial_base_nonzero"])
        # Option names are whole words: a longer/shorter word does not match.
        self.assertFalse(ORACLE.early_serial_decision("xearlyprintk=serial myconsole=uart8250,io,0x3f8")["early_serial_base_nonzero"])
        self.assertFalse(ORACLE.early_serial_decision("earlyprintkx=serial")["early_serial_base_nonzero"])
        # The last `console=` wins, like in the C parser.
        self.assertTrue(ORACLE.early_serial_decision("console=ttyS0 console=uart8250,io,0x3f8")["early_serial_base_nonzero"])
        self.assertFalse(ORACLE.early_serial_decision("console=uart8250,io,0x3f8 console=ttyS0")["early_serial_base_nonzero"])

    def test_cmdline_option_value(self) -> None:
        self.assertEqual(ORACLE.cmdline_option_value(b"a=1 b=2  b=3", "b"), "3")
        self.assertIsNone(ORACLE.cmdline_option_value(b"ab=1", "b"))
        self.assertIsNone(ORACLE.cmdline_option_value(b"b", "b"))
        self.assertEqual(ORACLE.cmdline_option_value(b"b=", "b"), "")


@unittest.skipUnless(OBJDUMP, "Xcode/LLVM objdump is required")
class InventoryTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def disassemble(self, code: bytes, vaddr: int = 0x1000200) -> list[dict]:
        path = self.tmp / "code.elf"
        path.write_bytes(INVENTORY.wrap_as_elf(code, vaddr))
        return INVENTORY.disassemble(path, OBJDUMP, vaddr, vaddr + len(code))

    def test_disassembly_of_the_decompressor_entry(self) -> None:
        insns = self.disassemble(bytes.fromhex("fcfa31c08ed88ed0"))
        self.assertEqual([i["bytes"].hex() for i in insns], ["fc", "fa", "31c0", "8ed8", "8ed0"])
        self.assertEqual(insns[0]["addr"], 0x1000200)
        self.assertEqual(insns[3]["asm"], "mov\tds, eax".replace("\t", " "))

    def test_long_instruction_bytes_are_not_polluted_by_the_tab_separator(self) -> None:
        insns = self.disassemble(bytes.fromhex("c78714760000" + "00000000"))
        self.assertEqual(len(insns), 1)
        self.assertEqual(len(insns[0]["bytes"]), 10)
        self.assertTrue(insns[0]["asm"].startswith("mov"))

    def test_form_description_and_family(self) -> None:
        mov = self.disassemble(bytes.fromhex("488b4308"))[0]
        info = INVENTORY.describe(mov)
        self.assertEqual(info["family"], "W mov r64,m")
        self.assertIn("m(qword)[base+disp]", info["key"])
        rep = INVENTORY.describe(self.disassemble(bytes.fromhex("f348a5"))[0])
        self.assertEqual(rep["family"], "REP W movsq m,m")
        vex = INVENTORY.describe(self.disassemble(bytes.fromhex("c4e2c1f7fb"))[0])
        self.assertTrue(vex["opcode"].startswith("vex-shlx"))

    def test_walk_follows_direct_branches_and_honours_decisions(self) -> None:
        # 0x1000 cmp eax,1 ; 0x1003 jb 0x100b ; 0x1005 mov eax,2 ; 0x100a ret ; 0x100b mov eax,3 ; 0x1010 ret
        code = bytes.fromhex("83f801" "7206" "b802000000" "c3" "b803000000" "c3")
        insns = {i["addr"]: i for i in self.disassemble(code, 0x1000)}
        self.assertEqual(sorted(insns), [0x1000, 0x1003, 0x1005, 0x100A, 0x100B, 0x1010])
        seen, _ = INVENTORY.walk(insns, [0x1000])
        self.assertEqual(seen, set(insns))
        seen_taken, _ = INVENTORY.walk(insns, [0x1000], decisions={0x1003: True})
        seen_fall, _ = INVENTORY.walk(insns, [0x1000], decisions={0x1003: False})
        self.assertEqual(seen_taken, {0x1000, 0x1003, 0x100B, 0x1010})
        self.assertEqual(seen_fall, {0x1000, 0x1003, 0x1005, 0x100A})

    def test_noreturn_call_stops_the_walk_and_is_detected(self) -> None:
        # 0x2000 call 0x200b ; 0x2005 mov eax,7 (padding after a noreturn call) ; 0x200a ret ; 0x200b hlt ; jmp 0x200b
        code = bytes.fromhex("e8" "06000000" "b807000000" "c3" "f4" "ebfd")
        insns = self.disassemble(code, 0x2000)
        noreturn = INVENTORY.find_noreturn(insns)
        self.assertEqual(noreturn, {0x200B})
        by_addr = {i["addr"]: i for i in insns}
        seen, _ = INVENTORY.walk(by_addr, [0x2000], noreturn=noreturn)
        self.assertEqual(seen, {0x2000, 0x200B})
        seen_without, _ = INVENTORY.walk(by_addr, [0x2000])
        self.assertIn(0x2005, seen_without)

    def test_status_rules(self) -> None:
        def status(raw: str, family_hint: str | None = None) -> dict:
            insn = self.disassemble(bytes.fromhex(raw))[0]
            row = INVENTORY.describe(insn)
            row.update({"example_bytes": raw, "example_asm": insn["asm"], "region": "decompressor_head_64.S_startup_64"})
            return STATUS.status_for(row)

        self.assertEqual(status("fc")["status"], "exact")  # CLD
        self.assertEqual(status("8ed8")["status"], "exact")  # MOV DS,AX
        self.assertEqual(status("8ed0")["status"], "absent")  # MOV SS,AX
        self.assertEqual(status("8ee0")["status"], "partial")  # MOV FS
        self.assertEqual(status("0f0155f6")["status"], "exact")  # LGDT [RBP-10]
        self.assertEqual(status("0f0110")["status"], "partial")  # LGDT [RAX]
        self.assertEqual(status("f30fbcc0")["status"], "absent")  # TZCNT
        self.assertEqual(status("c9")["status"], "absent")  # LEAVE
        self.assertEqual(status("488b4308")["status"], "family")  # MOV r64,[rbx+8]
        self.assertEqual(status("9d")["status"], "partial")  # POPFQ (CPL3 only in the contract)
        self.assertEqual(status("0fae38")["status"], "absent")  # CLFLUSH
        # An instruction no rule covers is `not_checked`, never guessed.
        self.assertEqual(status("0fc8")["status"], "not_checked")  # BSWAP EAX

    def test_every_rule_names_contract_evidence(self) -> None:
        for pattern, status, evidence in STATUS.RULES:
            self.assertIn(status, {"exact", "family", "partial", "absent", "not_checked"}, pattern)
            self.assertTrue(evidence, pattern)


@unittest.skipUnless(
    os.environ.get("OD_S4_38L2_REAL") == "1" and (ORACLE.DEFAULT_STAGE_DIR / ORACLE.VMLINUZ).is_file() and ZSTD and OBJDUMP,
    "set OD_S4_38L2_REAL=1 with the staged kernel to run",
)
class RealArtifactTest(unittest.TestCase):
    def test_real_payload_matches_vmlinux(self) -> None:
        with tempfile.TemporaryDirectory(prefix="s4-38l2-test-") as tmp:
            evidence = ORACLE.run_oracle(ORACLE.DEFAULT_STAGE_DIR, Path(tmp), ZSTD, ORACLE.DEFAULT_S435_EVIDENCE, None, "n")
        self.assertTrue(evidence["verdict"]["all_ok"], evidence["verdict"])


if __name__ == "__main__":
    unittest.main()
