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

    def test_machine_exact_profile_and_explicit_exclusion_are_distinct(self) -> None:
        registry = __import__("json").loads(analyzer.PROFILE_PATH.read_text())
        exact, excluded = analyzer.profile_matcher_maps(registry)
        load = analyzer.parse_row("  401000: 48 8b 03 mov rax,QWORD PTR [rbx]")
        syscall = analyzer.parse_row("  401003: 0f 05 syscall")
        byte_in = analyzer.parse_row("  401005: ec in al,dx")
        self.assertIn("S4-20b/d exact load", {x["task"] for x in analyzer.exact_profile_candidates(load, exact)})
        self.assertIn("S4-20c syscall", {x["task"] for x in analyzer.exact_profile_candidates(syscall, exact)})
        self.assertIsNotNone(analyzer.excluded_profile_match(byte_in, excluded))
        self.assertIsNotNone(analyzer.cpl0_encoding_candidate(byte_in))

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
