#!/usr/bin/env python3
"""Unit tests for the Stage 5 static ISA inventory parser and lock checks."""

from __future__ import annotations

import hashlib
import importlib.util
import lzma
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

MODULE_PATH = Path(__file__).with_name("analyze_stage5_isa.py")
SPEC = importlib.util.spec_from_file_location("analyze_stage5_isa", MODULE_PATH)
assert SPEC and SPEC.loader
analyzer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(analyzer)


class ParseObjdumpTests(unittest.TestCase):
    def test_register_and_memory_forms_keep_operand_width(self) -> None:
        mov = analyzer.parse_row("  401000: 48 89 e5 mov rbp,rsp")
        self.assertIsNotNone(mov)
        self.assertEqual(mov["signature"], "mov r64,r64")

        load = analyzer.parse_row("  401003: 48 8b 05 00 00 00 00 mov rax,QWORD PTR [rip+0x0] # 40100a")
        self.assertIsNotNone(load)
        self.assertEqual(load["signature"], "mov r64,mem64")

    def test_prefix_and_branch_form_are_preserved(self) -> None:
        row = analyzer.parse_row("  40100a: f3 a4 rep movs BYTE PTR es:[rdi],BYTE PTR ds:[rsi]")
        self.assertIsNotNone(row)
        self.assertEqual(row["signature"], "rep movs mem8,mem8")

        branch = analyzer.parse_row("  401010: e8 00 00 00 00 call 401015 <target>")
        self.assertIsNotNone(branch)
        self.assertEqual(branch["signature"], "call rel")

        conditional = analyzer.parse_row("  401015: 75 e9 jne 401000 <start>")
        self.assertIsNotNone(conditional)
        self.assertEqual(conditional["signature"], "jne rel")

    def test_exact_segment_register_profile_forms_are_not_merged(self) -> None:
        fs = analyzer.parse_row("  401020: 8e e0 mov fs,ax")
        gs = analyzer.parse_row("  401022: 8e e8 mov gs,ax")
        self.assertEqual(fs["signature"], "mov fs,r16")
        self.assertEqual(gs["signature"], "mov gs,r16")

    def test_non_instruction_and_unparseable_rows_are_rejected(self) -> None:
        self.assertIsNone(analyzer.parse_row("Disassembly of section .text:"))
        self.assertIsNone(analyzer.parse_row("  401000: <bad opcode>"))

    def test_full_width_objdump_rows_preserve_long_instructions(self) -> None:
        row = analyzer.parse_row("  401000: 48 b8 01 00 00 00 00 00 00 00 movabs rax,0x1")
        self.assertIsNotNone(row)
        self.assertEqual(len(row["byte_values"]), 10)
        self.assertEqual(analyzer.instruction_dimensions(row)["instruction_length"], 10)
        self.assertIn("--insn-width=16", analyzer.worker_command())

    def test_near_conditional_branch_has_no_modrm(self) -> None:
        row = analyzer.parse_row("  401000: 0f 85 24 00 00 00 jne 40102a <target>")
        self.assertEqual(row["signature"], "jne rel")
        view = analyzer.opcode_view(row)
        self.assertIsNone(view["modrm"])
        self.assertIsNone(analyzer.instruction_dimensions(row)["modrm"])

    def test_fs_sub_has_modrm_and_matches_bounded_l7_candidate(self) -> None:
        row = analyzer.parse_row("  401000: 64 48 2b 00 sub rax,QWORD PTR fs:[rax]")
        self.assertEqual(analyzer.opcode_view(row)["modrm"], 0x00)
        self.assertIn("S4-38L7j-FS-GS-qword-SUB", analyzer.cpl0_l7_candidates(row))

    def test_symbol_annotation_does_not_change_direct_branch_operand_class(self) -> None:
        row = analyzer.parse_row("  401000: e8 00 00 00 00 call 401005 <foo_ptr>")
        self.assertEqual(row["signature"], "call rel")
        self.assertEqual(analyzer.local_profile_candidate(row), "S2-17")
        jump = analyzer.parse_row("  401005: e9 00 00 00 00 jmp 40100a <target>")
        self.assertEqual(analyzer.local_profile_candidate(jump), "S2-16")

    def test_absolute_segment_memory_and_objdump_pseudo_prefixes(self) -> None:
        absolute_fs = analyzer.parse_row("  401000: 64 48 8b 04 25 28 00 00 00 mov rax,QWORD PTR fs:0x28")
        dims = analyzer.instruction_dimensions(absolute_fs)
        self.assertEqual(absolute_fs["signature"], "mov r64,mem64")
        self.assertEqual(dims["address"], {"form": "absolute", "size": 64, "segment": "fs"})

        nop = analyzer.parse_row("  401000: 66 66 2e 0f 1f 84 00 00 00 00 00 data16 cs nop WORD PTR [rax+rax*1+0x0]")
        self.assertEqual(nop["mnemonic"], "nop")
        self.assertEqual(nop["signature"], "nop mem16")
        self.assertEqual(analyzer.instruction_dimensions(nop)["prefix_bytes"], ["66", "66", "2E"])
        mov = analyzer.parse_row("  401000: 66 66 66 64 48 8b 04 25 00 00 00 00 data16 data16 data16 mov rax,QWORD PTR fs:0x0")
        self.assertEqual(mov["signature"], "mov r64,mem64")
        self.assertEqual(analyzer.instruction_dimensions(mov)["address"], {"form": "absolute", "size": 64, "segment": "fs"})

    def test_x87_memory_forms_keep_modrm_and_tbyte_width(self) -> None:
        fstp = analyzer.parse_row("  401000: db bd d0 fe ff ff fstp TBYTE PTR [rbp-0x130]")
        fild = analyzer.parse_row("  401006: df ad d0 fe ff ff fild QWORD PTR [rbp-0x130]")
        self.assertEqual(fstp["signature"], "fstp mem80")
        self.assertEqual(analyzer.opcode_view(fstp)["modrm"], 0xBD)
        self.assertEqual(fild["signature"], "fild mem64")
        self.assertEqual(analyzer.opcode_view(fild)["modrm"], 0xAD)

    def test_local_profile_match_uses_opcode_and_prefix_gate(self) -> None:
        mov = analyzer.parse_row("  401000: 48 8b 03 mov rax,QWORD PTR [rbx]")
        add = analyzer.parse_row("  401003: 83 c0 01 add eax,0x1")
        branch = analyzer.parse_row("  40100a: 75 f4 jne 401000 <start>")
        shift = analyzer.parse_row("  40100c: 48 d1 e0 shl rax,1")
        divide = analyzer.parse_row("  40100f: 48 f7 f1 div rcx")
        register_xchg = analyzer.parse_row("  401013: 48 87 c3 xchg rbx,rax")
        memory_xchg = analyzer.parse_row("  401016: 48 87 03 xchg QWORD PTR [rbx],rax")
        locked_memory_xchg = analyzer.parse_row("  401019: f0 48 87 03 lock xchg QWORD PTR [rbx],rax")
        locked = analyzer.parse_row("  401006: f0 48 01 03 lock add QWORD PTR [rbx],rax")
        fs_load = analyzer.parse_row("  40100a: 64 48 8b 00 mov rax,QWORD PTR fs:[rax]")
        self.assertEqual(analyzer.local_profile_candidate(mov), "S2-08")
        self.assertEqual(analyzer.local_profile_candidate(add), "S2-11")
        self.assertEqual(analyzer.local_profile_candidate(branch), "S2-16")
        self.assertEqual(analyzer.local_profile_candidate(shift), "S2-13")
        self.assertEqual(analyzer.local_profile_candidate(divide), "S2-14")
        self.assertEqual(analyzer.local_profile_candidate(register_xchg), "S2-10")
        self.assertEqual(analyzer.local_profile_candidate(memory_xchg), "S2-21")
        self.assertEqual(analyzer.local_profile_candidate(locked_memory_xchg), "S2-21")
        self.assertIsNone(analyzer.local_profile_candidate(locked))
        self.assertIsNone(analyzer.local_profile_candidate(fs_load))

    def test_machine_exact_profile_and_accepted_default_byte_pio_are_distinct(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        exact, excluded = analyzer.profile_matcher_maps(registry)
        load = analyzer.parse_row("  401000: 48 8b 03 mov rax,QWORD PTR [rbx]")
        syscall = analyzer.parse_row("  401003: 0f 05 syscall")
        byte_in = analyzer.parse_row("  401005: ec in al,dx")
        self.assertIn("S4-20b/d exact load", {x["task"] for x in analyzer.exact_profile_candidates(load, exact)})
        self.assertIn("S4-20c syscall", {x["task"] for x in analyzer.exact_profile_candidates(syscall, exact)})
        self.assertIsNone(analyzer.excluded_profile_match(byte_in, excluded))
        self.assertIn("S4-38L7x-byte-PIO", analyzer.cpl0_l7_candidates(byte_in))
        self.assertEqual(analyzer.local_profile_candidate(byte_in), None)
        byte_out = analyzer.parse_row("  401006: ee out dx,al")
        self.assertIn("S4-38L7x-byte-PIO", analyzer.cpl0_l7_candidates(byte_out))

    def test_s2_12_test_accumulator_and_s2_15a_sign_extension_are_local_only(self) -> None:
        for line in (
            "  401010: a8 01 test al,0x1",
            "  401012: a9 01 00 00 00 test eax,0x1",
        ):
            self.assertEqual(analyzer.local_profile_candidate(analyzer.parse_row(line)), "S2-12")
        for line in (
            "  401017: 98 cwde",
            "  401018: 48 98 cdqe",
            "  40101a: 99 cdq",
            "  40101b: 48 99 cqo",
        ):
            row = analyzer.parse_row(line)
            self.assertEqual(analyzer.local_profile_candidate(row), "S2-15a")
            self.assertEqual(analyzer.exact_profile_candidates(row, analyzer.profile_matcher_maps(__import__("json").loads(analyzer.PROFILE_PATH.read_text()))[0]), [])

    def test_s2_34_movups_local_memory_profile_candidates_are_exact_and_local_only(self) -> None:
        cases = (
            ("0f 10 00", "movups xmm0,XMMWORD PTR [rax]", "S2-34.4-MOVUPS-memory-load-RAX"),
            ("44 0f 10 00", "movups xmm8,XMMWORD PTR [rax]", "S2-34.4-MOVUPS-memory-load-RAX"),
            ("0f 11 00", "movups XMMWORD PTR [rax],xmm0", "S2-34.5-MOVUPS-memory-store-RAX"),
            ("44 0f 11 00", "movups XMMWORD PTR [rax],xmm8", "S2-34.5-MOVUPS-memory-store-RAX"),
            ("0f 10 43 01", "movups xmm0,XMMWORD PTR [rbx+0x1]", "S2-34.6a-MOVUPS-base-displacement"),
            ("0f 11 0b", "movups XMMWORD PTR [rbx],xmm1", "S2-34.6a-MOVUPS-base-displacement"),
            ("41 0f 10 00", "movups xmm0,XMMWORD PTR [r8]", "S2-34.6a-MOVUPS-base-displacement"),
            ("45 0f 11 00", "movups XMMWORD PTR [r8],xmm8", "S2-34.6a-MOVUPS-base-displacement"),
            ("41 0f 11 40 01", "movups XMMWORD PTR [r8+0x1],xmm0", "S2-34.6a-MOVUPS-base-displacement"),
            ("0f 10 05 01 00 00 00", "movups xmm0,XMMWORD PTR [rip+0x1]", "S2-34.6b-MOVUPS-RIP-relative"),
            ("44 0f 11 05 ff ff ff ff", "movups XMMWORD PTR [rip-0x1],xmm8", "S2-34.6b-MOVUPS-RIP-relative"),
            ("0f 10 44 24 01", "movups xmm0,XMMWORD PTR [rsp+0x1]", "S2-34.6c-MOVUPS-SIB"),
            ("47 0f 11 4c 24 01", "movups XMMWORD PTR [r12+r12+0x1],xmm9", "S2-34.6c-MOVUPS-SIB"),
        )
        exact, _ = analyzer.profile_matcher_maps(__import__("json").loads(analyzer.PROFILE_PATH.read_text()))
        for encoding, operands, expected in cases:
            row = analyzer.parse_row(f"  401080: {encoding} {operands}")
            self.assertEqual(analyzer.local_profile_candidate(row), expected, encoding)
            self.assertEqual(analyzer.exact_profile_candidates(row, exact), [], encoding)
        rejected = (
            ("0f 10 c1", "movups xmm0,xmm1"),  # register form belongs to other S2-34 tasks
            ("66 0f 10 00", "movupd xmm0,XMMWORD PTR [rax]"),
            ("f3 0f 10 00", "movss xmm0,DWORD PTR [rax]"),
            ("67 0f 10 43 01", "movups xmm0,XMMWORD PTR [ebx+0x1]"),
            ("64 0f 10 00", "movups xmm0,XMMWORD PTR fs:[rax]"),
            ("f0 0f 10 00", "lock movups xmm0,XMMWORD PTR [rax]"),
            ("48 0f 10 00", "movups xmm0,XMMWORD PTR [rax]"),
            ("42 0f 10 43 01", "movups xmm0,XMMWORD PTR [rbx+0x1]"),  # base profile rejects REX.X
            ("49 0f 10 05 01 00 00 00", "movups xmm0,XMMWORD PTR [rip+0x1]"),  # RIP form rejects REX.B
            ("48 0f 10 44 24 01", "movups xmm0,XMMWORD PTR [rsp+0x1]"),  # SIB form rejects REX.W
            ("c5 f8 10 00", "vmovups xmm0,XMMWORD PTR [rax]"),
            ("62 f1 7c 08 10 00", "vmovups xmm0,XMMWORD PTR [rax]"),
            ("0f 10 44 24", "movups xmm0,XMMWORD PTR [rsp+0x1]"),  # truncated SIB displacement
        )
        for encoding, operands in rejected:
            row = analyzer.parse_row(f"  401100: {encoding} {operands}")
            self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

    def test_s2_34_movups_registry_is_local_and_scoped_to_five_profiles(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        self.assertFalse(registry["registry_complete"])
        profiles = [p for p in registry["profiles"] if p["task"].startswith("S2-34.4/")]
        self.assertEqual(len(profiles), 1)
        profile = profiles[0]
        self.assertEqual(profile["evidence_scope"], "local_interpreter_profile")
        self.assertIsNone(profile["cpl"])
        self.assertIn("no native/JIT/physical or machine-CPL evidence", profile["gate"])
        self.assertEqual(
            [family["task"] for family in profile["families"]],
            ["S2-34.4", "S2-34.5", "S2-34.6a", "S2-34.6b", "S2-34.6c"],
        )

    def test_s2_35_1_pxor_matches_only_exact_local_xmm_register_forms(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        exact, _ = analyzer.profile_matcher_maps(registry)
        for destination in range(16):
            for source in range(16):
                rex = 0x40 | ((destination >> 3) << 2) | (source >> 3)
                modrm = 0xC0 | ((destination & 7) << 3) | (source & 7)
                prefixes = "66" if rex == 0x40 else f"66 {rex:02x}"
                encoding = f"{prefixes} 0f ef {modrm:02x}"
                operands = f"pxor xmm{destination},xmm{source}"
                row = analyzer.parse_row(f"  401200: {encoding} {operands}")
                self.assertEqual(row["signature"], "pxor xmm,xmm", encoding)
                self.assertEqual(analyzer.local_profile_candidate(row), "S2-35.1-PXOR-XMM-register-register", encoding)
                self.assertEqual(analyzer.exact_profile_candidates(row, exact), [], encoding)
        # The contract permits an explicit neutral REX byte as the optional REX form.
        row = analyzer.parse_row("  401300: 66 40 0f ef c1 pxor xmm0,xmm1")
        self.assertEqual(analyzer.local_profile_candidate(row), "S2-35.1-PXOR-XMM-register-register")

        rejected = (
            ("0f ef c1", "pxor mm0,mm1"),  # MMX encoding lacks mandatory 66.
            ("66 0f ef 01", "pxor QWORD PTR [rcx],mm0"),  # memory ModR/M.
            ("66 48 0f ef c1", "pxor xmm0,xmm1"),  # REX.W.
            ("66 42 0f ef c1", "pxor xmm0,xmm1"),  # REX.X.
            ("66 f2 0f ef c1", "pxor xmm0,xmm1"),
            ("66 f3 0f ef c1", "pxor xmm0,xmm1"),
            ("67 66 0f ef c1", "pxor xmm0,xmm1"),
            ("f0 66 0f ef c1", "lock pxor xmm0,xmm1"),
            ("64 66 0f ef c1", "pxor xmm0,xmm1"),
            ("66 66 0f ef c1", "pxor xmm0,xmm1"),
            ("66 41 44 0f ef c1", "pxor xmm0,xmm1"),
            ("41 66 0f ef c1", "pxor xmm0,xmm1"),  # REX must follow 66.
            ("66 0f ef c1 90", "pxor xmm0,xmm1"),  # no extra trailing byte.
            ("66 0f ef", "pxor xmm0,xmm1"),  # truncated ModR/M.
            ("c5 f9 ef c1", "vpxor xmm0,xmm0,xmm1"),
            ("62 f1 7d 08 ef c1", "vpxor xmm0,xmm0,xmm1"),
        )
        for encoding, operands in rejected:
            row = analyzer.parse_row(f"  401400: {encoding} {operands}")
            self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

    def test_s2_35_35_pshufd_matches_all_register_pairs_and_imm8_values(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        exact, _ = analyzer.profile_matcher_maps(registry)
        expected = "S2-35.35-PSHUFD-XMM-register-register"
        for destination in range(16):
            for source in range(16):
                rex = 0x40 | ((destination >> 3) << 2) | (source >> 3)
                self.assertIn(rex, {0x40, 0x41, 0x44, 0x45})
                modrm = 0xC0 | ((destination & 7) << 3) | (source & 7)
                for control in range(256):
                    encoding = f"66 {rex:02x} 0f 70 {modrm:02x} {control:02x}"
                    operands = f"pshufd xmm{destination},xmm{source},0x{control:x}"
                    row = analyzer.parse_row(f"  401600: {encoding} {operands}")
                    self.assertEqual(row["signature"], "pshufd xmm,xmm,imm", encoding)
                    self.assertEqual(analyzer.local_profile_candidate(row), expected, encoding)
                    self.assertEqual(analyzer.exact_profile_candidates(row, exact), [], encoding)

        # The REX prefix is optional when both registers are in XMM0-XMM7.
        for control in range(256):
            row = analyzer.parse_row(
                f"  401700: 66 0f 70 c1 {control:02x} pshufd xmm0,xmm1,0x{control:x}"
            )
            self.assertEqual(analyzer.local_profile_candidate(row), expected)

        rejected = (
            ("0f 70 c1 01", "pshufd mm0,mm1,0x1"),  # bare MMX form
            ("66 0f 70 c1", "pshufd xmm0,xmm1,0x1"),  # missing immediate byte
            ("66 0f 70 c1 01 90", "pshufd xmm0,xmm1,0x1"),  # extra trailing byte
            ("66 0f 70 01 01", "pshufd xmm0,XMMWORD PTR [rcx],0x1"),  # memory ModR/M
            ("66 0f 70 41 00 01", "pshufd xmm0,XMMWORD PTR [rcx+0x0],0x1"),
            ("66 0f 70 81 00 00 00 00 01", "pshufd xmm0,XMMWORD PTR [rcx+0x0],0x1"),
            ("66 48 0f 70 c1 01", "pshufd xmm0,xmm1,0x1"),  # REX.W
            ("66 42 0f 70 c1 01", "pshufd xmm0,xmm1,0x1"),  # REX.X
            ("66 46 0f 70 c1 01", "pshufd xmm0,xmm1,0x1"),  # REX.X plus REX.R
            ("66 66 0f 70 c1 01", "pshufd xmm0,xmm1,0x1"),
            ("66 f2 0f 70 c1 01", "pshufd xmm0,xmm1,0x1"),
            ("66 f3 0f 70 c1 01", "pshufd xmm0,xmm1,0x1"),
            ("41 66 0f 70 c1 01", "pshufd xmm0,xmm1,0x1"),  # REX before 66
            ("c5 f9 70 c1 01", "vpshufd xmm0,xmm1,0x1"),
            ("62 f1 7d 08 70 c1 01", "vpshufd xmm0,xmm1,0x1"),
            ("66 0f 71 c1 01", "pshufd xmm0,xmm1,0x1"),  # other opcode
        )
        for encoding, operands in rejected:
            row = analyzer.parse_row(f"  401800: {encoding} {operands}")
            self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

    def test_s2_35_35_registry_is_local_only_and_excludes_memory(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        self.assertFalse(registry["registry_complete"])
        profiles = [p for p in registry["profiles"] if p["task"] == "S2-35.35 PSHUFD XMM register-only"]
        self.assertEqual(len(profiles), 1)
        profile = profiles[0]
        self.assertEqual(profile["evidence_scope"], "local_interpreter_profile")
        self.assertIsNone(profile["cpl"])
        self.assertEqual(len(profile["families"]), 1)
        family = profile["families"][0]
        self.assertEqual(family["task"], "S2-35.35")
        self.assertIn("ModR/M.mod=3", family["encoding"])
        self.assertIn("any imm8", family["gate"])
        self.assertIn("REX.W/X", family["gate"])
        self.assertIn("no native/JIT/physical or machine-CPL evidence", profile["gate"])

    def test_s2_35_1_registry_is_local_interpreter_only(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        self.assertFalse(registry["registry_complete"])
        profiles = [p for p in registry["profiles"] if p["task"] == "S2-35.1 PXOR XMM register-only"]
        self.assertEqual(len(profiles), 1)
        profile = profiles[0]
        self.assertEqual(profile["evidence_scope"], "local_interpreter_profile")
        self.assertIsNone(profile["cpl"])
        self.assertIn("no native/JIT/physical or machine-CPL evidence", profile["gate"])
        self.assertEqual(len(profile["families"]), 1)
        self.assertTrue(profile["families"][0]["encoding"].startswith("66 [one optional REX.R/B] 0F EF /r; ModR/M.mod=3"))

    def test_reviewed_register_logic_profiles_match_exact_prefix_and_opcode_split(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        exact, _ = analyzer.profile_matcher_maps(registry)
        profiles = (
            (0x57, "xorps", "S2-34.7a-XORPS-register-register", False),
            (0x54, "andps", "S2-34.7b-ANDPS-register-register", False),
            (0x56, "orps", "S2-34.7c-ORPS-register-register", False),
            (0x55, "andnps", "S2-34.7d-ANDNPS-register-register", False),
            (0xDF, "pandn", "S2-35.50-PANDN-register-register", True),
            (0xEB, "por", "S2-35.51-POR-register-register", True),
        )
        allowed_rex = {0x40, 0x41, 0x44, 0x45}

        for opcode, mnemonic, task, mandatory_66 in profiles:
            for destination in range(16):
                for source in range(16):
                    rex = 0x40 | ((destination >> 3) << 2) | (source >> 3)
                    self.assertIn(rex, allowed_rex)
                    base_prefix = (0x66,) if mandatory_66 else ()
                    prefix_forms = [base_prefix + (rex,)]
                    if rex == 0x40:
                        prefix_forms.insert(0, base_prefix)
                    for prefixes in prefix_forms:
                        modrm = 0xC0 | ((destination & 7) << 3) | (source & 7)
                        encoding = " ".join(f"{value:02x}" for value in (*prefixes, 0x0F, opcode, modrm))
                        row = analyzer.parse_row(f"  401A00: {encoding} {mnemonic} xmm{destination},xmm{source}")
                        self.assertEqual(row["signature"], f"{mnemonic} xmm,xmm", encoding)
                        self.assertEqual(analyzer.local_profile_candidate(row), task, encoding)
                        self.assertEqual(analyzer.exact_profile_candidates(row, exact), [], encoding)

            prefix_forms = ((), (0x40,), (0x41,), (0x44,), (0x45,))
            if mandatory_66:
                prefix_forms = tuple((0x66, *prefix) for prefix in prefix_forms)
            # Every memory ModR/M encoding is excluded under each legal prefix shape.
            for modrm in range(0xC0):
                for prefixes in prefix_forms:
                    encoding = " ".join(f"{value:02x}" for value in (*prefixes, 0x0F, opcode, modrm))
                    row = analyzer.parse_row(f"  401B00: {encoding} {mnemonic} xmm0,XMMWORD PTR [rax]")
                    self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

        rejected = (
            # The reviewed SSE profiles are NP: mandatory 66 is outside their form.
            ("66 0f 57 c1", "xorps", "xmm0,xmm1"),
            ("66 0f 54 c1", "andps", "xmm0,xmm1"),
            ("66 0f 56 c1", "orps", "xmm0,xmm1"),
            ("66 0f 55 c1", "andnps", "xmm0,xmm1"),
            # PAND is implemented, but independent review is pending.
            ("66 0f db c1", "pand", "xmm0,xmm1"),
            ("0f db c1", "pand", "mm0,mm1"),
            # PANDN/POR require mandatory 66; bare encodings are MMX.
            ("0f df c1", "pandn", "mm0,mm1"),
            ("0f eb c1", "por", "mm0,mm1"),
        )
        for encoding, mnemonic, operands in rejected:
            row = analyzer.parse_row(f"  401C00: {encoding} {mnemonic} {operands}")
            self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

        for opcode, mnemonic, _, mandatory_66 in profiles:
            bad_prefixes = (
                ((0x66,) if not mandatory_66 else ()),
                (0xF0,), (0xF2,), (0xF3,), (0x67,), (0x26,), (0x2E,),
                (0x36,), (0x3E,), (0x64,), (0x65,), (0x40, 0x41), (0x41, 0x66),
                (0x66, 0x66) if mandatory_66 else (0x66, 0x40),
            )
            for prefixes in bad_prefixes:
                encoding = " ".join(f"{value:02x}" for value in (*prefixes, 0x0F, opcode, 0xC1))
                row = analyzer.parse_row(f"  401D00: {encoding} {mnemonic} xmm0,xmm1")
                self.assertIsNone(analyzer.local_profile_candidate(row), encoding)
            for rex in range(0x40, 0x50):
                if rex in allowed_rex:
                    continue
                prefixes = (0x66, rex) if mandatory_66 else (rex,)
                encoding = " ".join(f"{value:02x}" for value in (*prefixes, 0x0F, opcode, 0xC1))
                row = analyzer.parse_row(f"  401E00: {encoding} {mnemonic} xmm0,xmm1")
                self.assertIsNone(analyzer.local_profile_candidate(row), encoding)
            for other_opcode, _, _, other_mandatory_66 in profiles:
                if other_opcode == opcode:
                    continue
                prefixes = (0x66,) if other_mandatory_66 else ()
                encoding = " ".join(f"{value:02x}" for value in (*prefixes, 0x0F, other_opcode, 0xC1))
                row = analyzer.parse_row(f"  401F00: {encoding} {mnemonic} xmm0,xmm1")
                self.assertIsNone(analyzer.local_profile_candidate(row), encoding)
            for encoding in (
                f"0f {opcode:02x}", f"66 0f {opcode:02x}",
                f"66 0f {opcode:02x} c1 90", "c5 f8 57 c1", "62 f1 7c 08 57 c1",
            ):
                row = analyzer.parse_row(f"  402000: {encoding} {mnemonic} xmm0,xmm1")
                self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

    def test_reviewed_logic_registry_is_local_only_and_excludes_pending_pand(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        self.assertFalse(registry["registry_complete"])
        expected_groups = {
            "S2-34.7a–.7d legacy XMM register logic",
            "S2-35.50/.51 XMM register PANDN/POR",
        }
        expected_families = {
            "S2-34.7a", "S2-34.7b", "S2-34.7c", "S2-34.7d", "S2-35.50", "S2-35.51",
        }
        profiles = [profile for profile in registry["profiles"] if profile["task"] in expected_groups]
        self.assertEqual({profile["task"] for profile in profiles}, expected_groups)
        self.assertEqual({family["task"] for profile in profiles for family in profile["families"]}, expected_families)
        self.assertFalse(any("S2-35.49" in profile["task"] for profile in registry["profiles"]))
        for profile in profiles:
            self.assertEqual(profile["evidence_scope"], "local_interpreter_profile")
            self.assertIsNone(profile["cpl"])
            self.assertIn("machine-CPL evidence", profile["gate"])

    def test_s2_35_24_to_31_punpck_matches_only_all_exact_xmm_register_forms(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        exact, _ = analyzer.profile_matcher_maps(registry)
        allowed_rex = (None, 0x40, 0x41, 0x44, 0x45)

        for opcode, (mnemonic, task) in analyzer.PUNPCK_LOCAL_PROFILES.items():
            for destination in range(16):
                for source in range(16):
                    modrm = 0xC0 | ((destination & 7) << 3) | (source & 7)
                    for rex in allowed_rex:
                        prefix = "66" if rex is None else f"66 {rex:02x}"
                        encoding = f"{prefix} 0f {opcode:02x} {modrm:02x}"
                        operands = f"xmm{destination},xmm{source}"
                        row = analyzer.parse_row(f"  401500: {encoding} {mnemonic} {operands}")
                        self.assertEqual(row["signature"], f"{mnemonic} xmm,xmm", encoding)
                        self.assertEqual(analyzer.local_profile_candidate(row), task, encoding)
                        self.assertEqual(analyzer.exact_profile_candidates(row, exact), [], encoding)

            # Exercise all 256 ModR/M values under each legal prefix shape.
            # Only mod=3 is register-register; every memory addressing mode is denied.
            for modrm in range(0xC0):
                for rex in allowed_rex:
                    prefix = "66" if rex is None else f"66 {rex:02x}"
                    encoding = f"{prefix} 0f {opcode:02x} {modrm:02x}"
                    row = analyzer.parse_row(f"  401600: {encoding} {mnemonic} xmm0,XMMWORD PTR [rax]")
                    self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

            for rex in range(0x40, 0x50):
                if rex in {0x40, 0x41, 0x44, 0x45}:
                    continue
                encoding = f"66 {rex:02x} 0f {opcode:02x} c1"
                row = analyzer.parse_row(f"  401700: {encoding} {mnemonic} xmm0,xmm1")
                self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

            for encoding in (
                f"0f {opcode:02x} c1",  # mandatory 66 absent: MMX or unsupported.
                f"66 66 0f {opcode:02x} c1",
                f"66 67 0f {opcode:02x} c1",
                f"66 f0 0f {opcode:02x} c1",
                f"66 f2 0f {opcode:02x} c1",
                f"66 f3 0f {opcode:02x} c1",
                f"66 2e 0f {opcode:02x} c1",
                f"66 36 0f {opcode:02x} c1",
                f"66 3e 0f {opcode:02x} c1",
                f"66 26 0f {opcode:02x} c1",
                f"66 64 0f {opcode:02x} c1",
                f"66 65 0f {opcode:02x} c1",
                f"41 66 0f {opcode:02x} c1",  # REX before mandatory 66.
                f"66 41 44 0f {opcode:02x} c1",  # multiple REX bytes.
                f"66 0f {opcode:02x}",
                f"66 0f {opcode:02x} c1 90",
                f"c5 f9 {opcode:02x} c1",  # VEX.
                f"62 f1 7d 08 {opcode:02x} c1",  # EVEX.
            ):
                row = analyzer.parse_row(f"  401800: {encoding} {mnemonic} xmm0,xmm1")
                self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

            for other_opcode in analyzer.PUNPCK_LOCAL_PROFILES:
                if other_opcode == opcode:
                    continue
                encoding = f"66 0f {other_opcode:02x} c1"
                row = analyzer.parse_row(f"  401900: {encoding} {mnemonic} xmm0,xmm1")
                self.assertIsNone(analyzer.local_profile_candidate(row), encoding)

    def test_s2_35_24_to_31_registry_is_local_only_and_memory_free(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        self.assertFalse(registry["registry_complete"])
        profiles = [p for p in registry["profiles"] if p["task"] == "S2-35.24–.31 PUNPCK XMM register-only"]
        self.assertEqual(len(profiles), 1)
        profile = profiles[0]
        self.assertEqual(profile["evidence_scope"], "local_interpreter_profile")
        self.assertIsNone(profile["cpl"])
        self.assertIn("no CPUID, exception-delivery, native/JIT, physical-execution, or machine-CPL evidence", profile["gate"])
        self.assertEqual([f["task"] for f in profile["families"]], [
            "S2-35.24", "S2-35.25", "S2-35.26", "S2-35.27",
            "S2-35.28", "S2-35.29", "S2-35.30", "S2-35.31",
        ])
        for family in profile["families"]:
            self.assertIn("ModR/M.mod=3", family["encoding"])
            self.assertIn("memory operands", family["gate"])

    def test_l7x_as_candidates_are_cpl0_only_and_index_safe(self) -> None:
        cases = (
            ("f0 0f b1 0b", "lock cmpxchg DWORD PTR [rbx],ecx", "S4-38L7y-LOCK-CMPXCHG-dword"),
            ("f0 83 03 01", "lock add DWORD PTR [rbx],0x1", "S4-38L7z-LOCK-ADD-memory-immediate"),
            ("65 48 39 03", "cmp QWORD PTR gs:[rbx],rax", "S4-38L7aa-FS-GS-CMP"),
            ("0f 01 3f", "invlpg FWORD PTR [rdi]", "S4-38L7ab-INVLPG-memory"),
            ("65 f7 03 ff ff ff 7f", "test DWORD PTR gs:[rbx],0x7fffffff", "S4-38L7ac-FS-GS-TEST32-64"),
            ("65 0f b7 3b", "movzx edi,WORD PTR gs:[rbx]", "S4-38L7ad-FS-GS-MOVZX-word"),
            ("65 c6 03 01", "mov BYTE PTR gs:[rbx],0x1", "S4-38L7ae-FS-GS-MOV-imm8"),
            ("2e e8 00 00 00 00", "cs call 0x401040", "S4-38L7af-prefixed-CALL-rel32"),
            ("f0 80 23 fe", "lock and BYTE PTR [rbx],0xfe", "S4-38L7ag-LOCK-AND-imm8"),
            ("64 83 3b 01", "cmp DWORD PTR fs:[rbx],0x1", "S4-38L7ah-FS-GS-CMP-imm8"),
            ("f0 01 0b", "lock add DWORD PTR [rbx],ecx", "S4-38L7ai-LOCK-ADD-register"),
            ("64 c7 03 01 00 00 00", "mov DWORD PTR fs:[rbx],0x1", "S4-38L7aj-FS-GS-MOV-immediate"),
            ("65 48 63 03", "movsxd rax,DWORD PTR gs:[rbx]", "S4-38L7ak-FS-GS-MOVSXD"),
            ("64 0f 01 03", "sgdt FWORD PTR fs:[rbx]", "S4-38L7al-FS-GS-SGDT"),
            ("0f 00 d8", "ltr ax", "S4-38L7am-LTR-register"),
            ("65 89 0b", "mov DWORD PTR gs:[rbx],ecx", "S4-38L7an-FS-GS-MOV-store"),
        )
        for i, (encoding, operands, task) in enumerate(cases):
            row = analyzer.parse_row(f"  401100: {encoding} {operands}")
            self.assertIn(task, analyzer.cpl0_l7_candidates(row), task)
            self.assertNotEqual(analyzer.local_profile_candidate(row), task)
        wrmsr = analyzer.parse_row("  401200: 0f 30 wrmsr")
        matches = analyzer.cpl0_l7_candidates(wrmsr)
        self.assertIn("S4-38L7ao-as-WRMSR-index-unobserved", matches)
        self.assertNotIn("S4-38L7f-WRMSR-EFER", matches)
        self.assertNotIn("S4-38L7ao-WRMSR-STAR", matches)

    def test_registry_records_l7as_without_claiming_ecx_index_or_complete_allowlist(self) -> None:
        import json
        registry = json.loads(analyzer.PROFILE_PATH.read_text())
        self.assertFalse(registry["registry_complete"])
        self.assertEqual(registry["explicit_profile_exclusions"], [])
        cpl0 = next(x for x in registry["profiles"] if x.get("task") == "S2-45 / Stage 2 CPL0 composed profile")
        family_tasks = {family["task"] for family in cpl0["families"]}
        accepted_l7 = {f"S4-38L7{suffix}" for suffix in ("x", "y", "z", "aa", "ab", "ac", "ad", "ae", "af", "ag", "ah", "ai", "aj", "ak", "al", "am", "an", "ao", "ap", "aq", "ar", "as")}
        self.assertTrue(accepted_l7.issubset(family_tasks))
        self.assertEqual(cpl0["evidence_scope"], "composed_cpl0_profile")
        msr = next(family for family in cpl0["families"] if family["task"] == "S4-38L7ar")
        self.assertIn("ECX index dynamic and not present", msr["gate"])
        aa = next(family for family in cpl0["families"] if family["task"] == "S4-38L7aa")
        al = next(family for family in cpl0["families"] if family["task"] == "S4-38L7al")
        self.assertIn("only opcode 39", aa["gate"])
        self.assertNotIn("3B", aa["encoding"])
        self.assertIn("empty", al["gate"])
        self.assertIn("non-W REX", al["gate"])

    def test_encoding_dimensions_preserve_l7_prefix_opcode_modrm_and_address(self) -> None:
        row = analyzer.parse_row("  401020: 64 48 8b 00 mov rax,QWORD PTR fs:[rax]")
        dims = analyzer.instruction_dimensions(row)
        self.assertEqual(dims["prefix_bytes"], ["64", "48"])
        self.assertEqual(dims["opcode_bytes"], ["8B"])
        self.assertEqual(dims["modrm"], {"raw": "00", "mod": 0, "reg": 0, "rm": 0})
        self.assertEqual(dims["address"], {"form": "base_only", "size": 64, "segment": "fs"})
        self.assertIn("S4-38L7i-FS-GS-qword-load", analyzer.cpl0_l7_candidates(row))

    def test_l7_locked_form_requires_memory_and_lock_prefix(self) -> None:
        locked = analyzer.parse_row("  401030: f0 48 0f b1 0b lock cmpxchg QWORD PTR [rbx],rcx")
        register = analyzer.parse_row("  401035: f0 48 0f b1 cb lock cmpxchg rbx,rcx")
        self.assertIn("S4-38L7k-LOCK-CMPXCHG-qword", analyzer.cpl0_l7_candidates(locked))
        self.assertEqual(analyzer.local_profile_candidate(locked), "S2-21")
        self.assertNotIn("S4-38L7k-LOCK-CMPXCHG-qword", analyzer.cpl0_l7_candidates(register))

    def test_l7y_dword_cmpxchg_rejects_address_size_override(self) -> None:
        accepted = analyzer.parse_row("  401037: f0 44 0f b1 0b lock cmpxchg DWORD PTR [rbx],r9d")
        accepted_rex_b = analyzer.parse_row("  401038: f0 45 0f b1 0b lock cmpxchg DWORD PTR [r11],r9d")
        address32 = analyzer.parse_row("  40103c: 67 f0 0f b1 0b lock cmpxchg DWORD PTR [ebx],ecx")
        operand16 = analyzer.parse_row("  40103d: 66 f0 0f b1 0b lock cmpxchg WORD PTR [rbx],cx")
        repeated_lock = analyzer.parse_row("  40103e: f0 f0 0f b1 0b lock cmpxchg DWORD PTR [rbx],ecx")
        segment = analyzer.parse_row("  40103f: 64 f0 0f b1 0b lock cmpxchg DWORD PTR fs:[rbx],ecx")
        rep = analyzer.parse_row("  40103a: f3 f0 0f b1 0b lock cmpxchg DWORD PTR [rbx],ecx")
        rex_not_last = analyzer.parse_row("  40103b: f0 48 66 0f b1 0b lock cmpxchg DWORD PTR [rbx],rcx")
        for row in (accepted, accepted_rex_b):
            self.assertIn("S4-38L7y-LOCK-CMPXCHG-dword", analyzer.cpl0_l7_candidates(row))
        for row in (address32, operand16, repeated_lock, segment, rep, rex_not_last):
            self.assertNotIn("S4-38L7y-LOCK-CMPXCHG-dword", analyzer.cpl0_l7_candidates(row))

    def test_l7z_lock_add_immediate_has_exact_prefix_contract(self) -> None:
        accepted = (
            ("f0 80 03 01", "lock add BYTE PTR [rbx],0x1"),
            ("f0 81 03 01 00 00 00", "lock add DWORD PTR [rbx],0x1"),
            ("f0 83 03 01", "lock add DWORD PTR [rbx],0x1"),
            ("f0 48 83 03 01", "lock add QWORD PTR [rbx],0x1"),
        )
        for encoding, operands in (
            ("67 f0 83 03 01", "lock add DWORD PTR [ebx],0x1"),
            ("66 f0 83 03 01", "lock add WORD PTR [rbx],0x1"),
            ("f0 f0 83 03 01", "lock add DWORD PTR [rbx],0x1"),
            ("64 f0 83 03 01", "lock add DWORD PTR fs:[rbx],0x1"),
            ("f0 48 66 83 03 01", "lock add DWORD PTR [rbx],0x1"),
            ("48 f0 83 03 01", "lock add DWORD PTR [rbx],0x1"),
            ("f0 83 0b 01", "lock or DWORD PTR [rbx],0x1"),
        ):
            row = analyzer.parse_row(f"  401045: {encoding} {operands}")
            self.assertNotIn("S4-38L7z-LOCK-ADD-memory-immediate", analyzer.cpl0_l7_candidates(row))
        for encoding, operands in accepted:
            row = analyzer.parse_row(f"  401040: {encoding} {operands}")
            self.assertIn("S4-38L7z-LOCK-ADD-memory-immediate", analyzer.cpl0_l7_candidates(row))

    def test_l7ag_lock_and_immediate_has_exact_prefix_contract(self) -> None:
        accepted = (
            ("f0 80 23 fe", "lock and BYTE PTR [rbx],0xfe"),
            ("f0 48 80 23 fe", "lock and BYTE PTR [rbx],0xfe"),
        )
        for encoding, operands in (
            ("67 f0 80 23 fe", "lock and BYTE PTR [ebx],0xfe"),
            ("66 f0 80 23 fe", "lock and BYTE PTR [rbx],0xfe"),
            ("f0 f0 80 23 fe", "lock and BYTE PTR [rbx],0xfe"),
            ("65 f0 80 23 fe", "lock and BYTE PTR gs:[rbx],0xfe"),
            ("f0 48 66 80 23 fe", "lock and BYTE PTR [rbx],0xfe"),
            ("f0 81 23 fe 00 00 00", "lock and DWORD PTR [rbx],0xfe"),
            ("f0 83 23 fe", "lock and DWORD PTR [rbx],0xfe"),
            ("f0 80 e3 fe", "lock and bl,0xfe"),
            ("48 f0 80 23 fe", "lock and BYTE PTR [rbx],0xfe"),
        ):
            row = analyzer.parse_row(f"  401055: {encoding} {operands}")
            self.assertNotIn("S4-38L7ag-LOCK-AND-imm8", analyzer.cpl0_l7_candidates(row))
        for encoding, operands in accepted:
            row = analyzer.parse_row(f"  401050: {encoding} {operands}")
            self.assertIn("S4-38L7ag-LOCK-AND-imm8", analyzer.cpl0_l7_candidates(row))

    def test_l7ai_lock_add_register_has_exact_prefix_contract(self) -> None:
        accepted = (
            ("f0 01 13", "lock add DWORD PTR [rbx],edx"),
            ("f0 48 01 0b", "lock add QWORD PTR [rbx],rcx"),
            ("f0 44 01 0b", "lock add DWORD PTR [rbx],r9d"),
        )
        for encoding, operands in (
            ("67 f0 01 0b", "lock add DWORD PTR [ebx],ecx"),
            ("66 f0 01 0b", "lock add WORD PTR [rbx],cx"),
            ("f0 f0 01 0b", "lock add DWORD PTR [rbx],ecx"),
            ("64 f0 01 0b", "lock add DWORD PTR fs:[rbx],ecx"),
            ("48 f0 01 0b", "lock add QWORD PTR [rbx],rcx"),
            ("f0 48 66 01 0b", "lock add QWORD PTR [rbx],rcx"),
            ("f0 00 0b", "lock add BYTE PTR [rbx],cl"),
            ("f0 03 0b", "lock add ecx,DWORD PTR [rbx]"),
            ("f0 01 cb", "lock add ebx,ecx"),
        ):
            row = analyzer.parse_row(f"  401065: {encoding} {operands}")
            self.assertNotIn("S4-38L7ai-LOCK-ADD-register", analyzer.cpl0_l7_candidates(row))
        for encoding, operands in accepted:
            row = analyzer.parse_row(f"  401060: {encoding} {operands}")
            self.assertIn("S4-38L7ai-LOCK-ADD-register", analyzer.cpl0_l7_candidates(row))

    def test_l7ac_fs_gs_test_rejects_lock_and_accepts_widths(self) -> None:
        dword = analyzer.parse_row("  401041: 65 f7 03 ff ff ff 7f test DWORD PTR gs:[rbx],0x7fffffff")
        qword = analyzer.parse_row("  401048: 65 48 f7 03 ff ff ff 7f test QWORD PTR gs:[rbx],0x7fffffff")
        locked = analyzer.parse_row("  401051: f0 65 f7 03 ff ff ff 7f lock test DWORD PTR gs:[rbx],0x7fffffff")
        for row in (dword, qword):
            self.assertIn("S4-38L7ac-FS-GS-TEST32-64", analyzer.cpl0_l7_candidates(row))
        self.assertNotIn("S4-38L7ac-FS-GS-TEST32-64", analyzer.cpl0_l7_candidates(locked))

    def test_l7am_ltr_accepts_non_w_rex_but_rejects_other_prefixes(self) -> None:
        plain = analyzer.parse_row("  401055: 0f 00 d8 ltr ax")
        rex_b = analyzer.parse_row("  401058: 41 0f 00 d8 ltr r8w")
        rex_r = analyzer.parse_row("  401059: 44 0f 00 d8 ltr ax")
        rex_x = analyzer.parse_row("  40105a: 45 0f 00 d8 ltr r8w")
        operand_size = analyzer.parse_row("  40105c: 66 0f 00 d8 ltr ax")
        address_size = analyzer.parse_row("  40105d: 67 0f 00 d8 ltr ax")
        rex_w = analyzer.parse_row("  401060: 48 0f 00 d8 ltr ax")
        segment = analyzer.parse_row("  401061: 64 0f 00 d8 ltr ax")
        lock = analyzer.parse_row("  401062: f0 0f 00 d8 ltr ax")
        memory = analyzer.parse_row("  401063: 0f 00 18 ltr WORD PTR [rax]")
        for row in (plain, rex_b, rex_r, rex_x):
            self.assertIn("S4-38L7am-LTR-register", analyzer.cpl0_l7_candidates(row))
        for row in (operand_size, address_size, rex_w, segment, lock, memory):
            self.assertNotIn("S4-38L7am-LTR-register", analyzer.cpl0_l7_candidates(row))

    def test_l7_segmented_candidates_obey_each_exact_prefix_matrix(self) -> None:
        def row(encoding: str, operands: str = "") -> dict:
            return analyzer.parse_row(f"  401070: {encoding} {operands}")

        # L7aa is only memory-destination CMP 39 /r; reverse direction 3B is excluded.
        for encoding in ("65 48 39 03", "64 45 39 0b"):
            self.assertIn("S4-38L7aa-FS-GS-CMP", analyzer.cpl0_l7_candidates(row(encoding, "cmp QWORD PTR gs:[rbx],rax")))
        for encoding, operands in (("67 65 48 39 03", "cmp QWORD PTR gs:[ebx],rax"), ("65 66 39 03", "cmp WORD PTR gs:[rbx],ax"), ("65 48 3b 03", "cmp rax,QWORD PTR gs:[rbx]")):
            self.assertNotIn("S4-38L7aa-FS-GS-CMP", analyzer.cpl0_l7_candidates(row(encoding, operands)))

        # L7ab: unprefixed, one non-W REX, one segment, or segment then non-W REX.
        for encoding in ("0f 01 3f", "40 0f 01 3f", "65 0f 01 3f", "64 45 0f 01 3f"):
            self.assertIn("S4-38L7ab-INVLPG-memory", analyzer.cpl0_l7_candidates(row(encoding, "invlpg FWORD PTR [rdi]")))
        for encoding in ("67 0f 01 3f", "48 0f 01 3f", "f0 0f 01 3f", "2e 0f 01 3f", "64 65 0f 01 3f"):
            self.assertNotIn("S4-38L7ab-INVLPG-memory", analyzer.cpl0_l7_candidates(row(encoding, "invlpg FWORD PTR [rdi]")))

        # L7ad / ae / ah / aj use exactly one FS/GS then optional immediately-final REX.
        candidate_rows = (
            ("S4-38L7ad-FS-GS-MOVZX-word", "65 48 0f b7 3b", "movzx rdi,WORD PTR gs:[rbx]", "67 65 48 0f b7 3b", "movzx rdi,WORD PTR gs:[ebx]"),
            ("S4-38L7ae-FS-GS-MOV-imm8", "64 40 c6 03 01", "mov BYTE PTR fs:[rbx],0x1", "67 64 40 c6 03 01", "mov BYTE PTR fs:[ebx],0x1"),
            ("S4-38L7ah-FS-GS-CMP-imm8", "64 48 83 3b 01", "cmp QWORD PTR fs:[rbx],0x1", "67 64 48 83 3b 01", "cmp QWORD PTR fs:[ebx],0x1"),
            ("S4-38L7aj-FS-GS-MOV-immediate", "65 48 c7 03 01 00 00 00", "mov QWORD PTR gs:[rbx],0x1", "67 65 48 c7 03 01 00 00 00", "mov QWORD PTR gs:[ebx],0x1"),
        )
        for task, allowed, allowed_ops, rejected, rejected_ops in candidate_rows:
            self.assertIn(task, analyzer.cpl0_l7_candidates(row(allowed, allowed_ops)))
            self.assertNotIn(task, analyzer.cpl0_l7_candidates(row(rejected, rejected_ops)))

        # L7ak requires exactly segment + REX.W; L7al also accepts no prefix or non-W REX.
        self.assertIn("S4-38L7ak-FS-GS-MOVSXD", analyzer.cpl0_l7_candidates(row("65 48 63 03", "movsxd rax,DWORD PTR gs:[rbx]")))
        for encoding in ("65 63 03", "67 65 48 63 03", "65 48 66 63 03"):
            self.assertNotIn("S4-38L7ak-FS-GS-MOVSXD", analyzer.cpl0_l7_candidates(row(encoding, "movsxd rax,DWORD PTR gs:[rbx]")))
        for encoding in ("0f 01 00", "40 0f 01 00", "65 0f 01 00", "64 45 0f 01 00"):
            self.assertIn("S4-38L7al-FS-GS-SGDT", analyzer.cpl0_l7_candidates(row(encoding, "sgdt [rax]")))
        for encoding in ("67 0f 01 00", "48 0f 01 00", "f0 65 0f 01 00"):
            self.assertNotIn("S4-38L7al-FS-GS-SGDT", analyzer.cpl0_l7_candidates(row(encoding, "sgdt [rax]")))

        # L7an additionally retains only the documented [early REX, segment, final REX.W] qword case.
        stale_qword = row("48 65 48 89 03", "mov QWORD PTR gs:[rbx],rax")
        self.assertIn("S4-38L7an-FS-GS-MOV-store", analyzer.cpl0_l7_candidates(stale_qword))
        self.assertNotIn("S4-38L7an-FS-GS-MOV-store", analyzer.cpl0_l7_candidates(row("67 65 89 0b", "mov DWORD PTR gs:[ebx],ecx")))

    def test_l7_uses_effective_segment_prefix_and_exact_width_gate(self) -> None:
        effective_fs = analyzer.parse_row("  401040: 2e 64 48 8b 00 mov rax,QWORD PTR fs:[rax]")
        overridden_fs = analyzer.parse_row("  401045: 64 2e 48 8b 00 mov rax,QWORD PTR cs:[rax]")
        qword_test16 = analyzer.parse_row("  40104a: 64 66 f7 00 01 00 test WORD PTR fs:[rax],0x1")
        self.assertIn("S4-38L7i-FS-GS-qword-load", analyzer.cpl0_l7_candidates(effective_fs))
        self.assertNotIn("S4-38L7i-FS-GS-qword-load", analyzer.cpl0_l7_candidates(overridden_fs))
        self.assertIn("S4-38L7r-FS-GS-TEST16", analyzer.cpl0_l7_candidates(qword_test16))

    def test_encoding_key_distinguishes_prefix_order_address_and_modrm(self) -> None:
        fs = analyzer.parse_row("  401050: 64 48 8b 00 mov rax,QWORD PTR fs:[rax]")
        fs_reordered = analyzer.parse_row("  401055: 48 64 8b 00 mov rax,QWORD PTR fs:[rax]")
        gs = analyzer.parse_row("  40105a: 65 48 8b 00 mov rax,QWORD PTR gs:[rax]")
        keys = {analyzer.instruction_dimensions(row)["encoding_key"] for row in (fs, fs_reordered, gs)}
        self.assertEqual(len(keys), 3)
        self.assertEqual(analyzer.instruction_dimensions(fs)["address"]["segment"], "fs")
        self.assertEqual(analyzer.instruction_dimensions(gs)["address"]["segment"], "gs")

    def test_cpl0_misc_candidate_is_bounded_to_memory_nop(self) -> None:
        memory_nop = analyzer.parse_row("  401060: 0f 1f 00 nop DWORD PTR [rax]")
        register_nop = analyzer.parse_row("  401063: 0f 1f c0 nop eax")
        self.assertEqual(analyzer.cpl0_misc_candidates(memory_nop), ["S4-38L4d-multi-byte-memory-NOP"])
        self.assertNotIn("S4-38L4d-multi-byte-memory-NOP", analyzer.cpl0_misc_candidates(register_nop))

    def test_optional_e_static_elf_is_a_separate_static_input(self) -> None:
        specs = analyzer.binary_specs(Path("work/stage5/s5-21/stage5-e-static.elf"))
        self.assertEqual(len(specs), len(analyzer.BINARIES) + 1)
        self.assertEqual(specs[-1], ("stage5-e-static", "external:stage5-e-static.elf", "external"))
        command = analyzer.worker_command(specs)
        self.assertIn("cp /extra/stage5-e-static.elf /tmp/s5-21-stage5-e-static", command)
        self.assertIn("x86_64-linux-gnu-objdump -d --insn-width=16 -M intel /tmp/s5-21-stage5-e-static", command)
        self.assertNotIn("qemu", command)
        self.assertNotIn("chroot", command)

    def test_static_elf_metadata_preserves_empty_build_id(self) -> None:
        index, record = analyzer.parse_meta("S5META\t8\tstage5-e-static\texternal:stage5-e-static.elf\t4096\tabc123\tAdvanced Micro Devices X86-64\t\n")
        self.assertEqual(index, 8)
        self.assertEqual(record["name"], "stage5-e-static")
        self.assertEqual(record["build_id"], "")

    def test_analyzer_rejects_copied_e_elf_hash_mismatch(self) -> None:
        class FakeProcess:
            stdout = iter(["S5META\t8\tstage5-e-static\texternal:stage5-e-static.elf\t16504\t" + "0" * 64 + "\tAdvanced Micro Devices X86-64\t\n"])

        with patch.object(analyzer, "verify_cache"), \
             patch.object(analyzer, "validate_stage5_e_static_elf"), \
             patch.object(analyzer.subprocess, "Popen", return_value=FakeProcess()):
            with self.assertRaisesRegex(RuntimeError, "copied Stage 5 E ELF hash"):
                analyzer.analyze(Path("unused-image"), Path("unused-cache"), {"tool_version": "pinned"}, Path("work/stage5/e.elf"))

    def test_e_static_summary_does_not_infer_runtime_generated_code(self) -> None:
        summary = analyzer.scenario_summaries([{
            "name": "stage5-e-static",
            "instruction_count": 123,
            "classification_frequencies": {"unclassified": 100, "proven_supported": 23},
        }])
        self.assertEqual(summary["E"]["status"], "static_executable_only")
        self.assertEqual(summary["E"]["static_instruction_count"], 123)
        self.assertEqual(summary["E"]["runtime_generated_code"]["status"], "not_inferred")
        self.assertEqual(summary["E"]["runtime_generated_code"]["frequency"], "not_inferred")

    def test_lgdt_lidt_require_0f01_group_memory_encoding(self) -> None:
        lgdt = analyzer.parse_row("  401070: 0f 01 10 lgdt FWORD PTR [rax]")
        lidt = analyzer.parse_row("  401073: 0f 01 18 lidt FWORD PTR [rax]")
        arithmetic = analyzer.parse_row("  401076: 01 10 add DWORD PTR [rax],edx")
        self.assertIn("S2-47a/49-LGDT-LIDT", analyzer.cpl0_misc_candidates(lgdt))
        self.assertIn("S2-47a/49-LGDT-LIDT", analyzer.cpl0_misc_candidates(lidt))
        self.assertNotIn("S2-47a/49-LGDT-LIDT", analyzer.cpl0_misc_candidates(arithmetic))


class ToolLockTests(unittest.TestCase):
    def test_cache_hashes_must_match_authenticated_package_stanza(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp)
            package_bytes = b"pinned package bytes"
            package_path = cache / "tool_1_arm64.deb"
            package_path.write_bytes(package_bytes)
            sha256 = hashlib.sha256(package_bytes).hexdigest()
            sha512 = hashlib.sha512(package_bytes).hexdigest()
            index_text = (
                "Package: tool\nVersion: 1\nArchitecture: arm64\n"
                "Filename: pool/main/t/tool_1_arm64.deb\n"
                f"SHA256: {sha256}\nSHA512: {sha512}\nSize: {len(package_bytes)}\n\n"
            )
            (cache / "main-arm64-Packages.xz").write_bytes(lzma.compress(index_text.encode()))
            index_hash = analyzer.digest(cache / "main-arm64-Packages.xz")
            lock = {
                "signed_packages_xz_sha256": index_hash,
                "packages": [{
                    "filename": package_path.name,
                    "package": "tool",
                    "version": "1",
                    "architecture": "arm64",
                    "uri": "https://example.test/pool/main/t/tool_1_arm64.deb",
                    "sha256": sha256,
                    "sha512": sha512,
                }],
            }
            self.assertEqual(analyzer.verify_cache(cache, lock), [package_path])
            package_path.write_bytes(b"replaced")
            with self.assertRaisesRegex(ValueError, "package hash mismatch"):
                analyzer.verify_cache(cache, lock)


if __name__ == "__main__":
    unittest.main()
