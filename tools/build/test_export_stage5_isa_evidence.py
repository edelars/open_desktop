#!/usr/bin/env python3
"""Tests for Stage 5 normalized ISA evidence aggregation."""

from __future__ import annotations

import importlib.util
import gzip
import hashlib
import json
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("export_stage5_isa_evidence.py")
SPEC = importlib.util.spec_from_file_location("export_stage5_isa_evidence", MODULE_PATH)
assert SPEC and SPEC.loader
exporter = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(exporter)
ANALYZER_SPEC = importlib.util.spec_from_file_location("analyze_stage5_isa", Path(__file__).with_name("analyze_stage5_isa.py"))
assert ANALYZER_SPEC and ANALYZER_SPEC.loader
analyzer = importlib.util.module_from_spec(ANALYZER_SPEC)
ANALYZER_SPEC.loader.exec_module(analyzer)


def exact_form(prefix, modrm, address, count, binary, classification):
    return {
        "dimensions": {
            "prefix_bytes": prefix,
            "opcode_bytes": ["8B"],
            "modrm": {"raw": modrm, "mod": 0, "reg": 0, "rm": 0},
            "address": address,
            "operand_signature": "mov r64,mem64",
        },
        "count": count,
        "binary_frequencies": {binary: count},
        "classification_frequencies": {classification: count},
        "evidence_scope_frequencies": {"local_interpreter_profile": count} if classification == "proven_supported" else {},
        "profile_match_frequencies": {},
        "conditional_candidate_frequencies": {},
    }


class NormalizedFormTests(unittest.TestCase):
    def test_saved_inventory_manifests_distinguish_coarse_and_normalized_forms(self) -> None:
        repo_root = Path(__file__).resolve().parents[2]
        data_root = repo_root / "docs/reports/data/stage5"
        pairs = (
            (
                "s5-21-repeatability.json",
                "s5-21-isa-inventory.json.gz",
                "full_inventory",
                "compact_evidence",
            ),
            (
                "s5-21-e-static-build-pair.json",
                "s5-21-isa-inventory-e-static.json.gz",
                None,
                "static_analysis",
            ),
        )
        for manifest_name, compact_name, full_key, compact_key in pairs:
            with self.subTest(manifest=manifest_name):
                manifest_path = data_root / manifest_name
                compact_path = data_root / compact_name
                manifest = json.loads(manifest_path.read_text())
                compact = json.loads(gzip.decompress(compact_path.read_bytes()))
                fields = manifest[full_key] if full_key else manifest[compact_key]
                compact_fields = manifest[compact_key] if full_key else fields

                self.assertEqual(compact["normalized_instruction_form_count"], 1269)
                self.assertEqual(len(compact["normalized_instruction_forms"]), 1269)
                self.assertEqual(fields["normalized_instruction_forms"], 1269)
                self.assertEqual(fields["coarse_operand_signatures"], 560)
                self.assertEqual(fields["classified_normalized_forms"], 1037)
                self.assertEqual(fields["unclassified_normalized_forms"], 232)
                pshufd_forms = [
                    row for row in compact["normalized_instruction_forms"]
                    if row["instruction_form"]["operand_signature"] == "pshufd xmm,xmm,imm"
                ]
                self.assertEqual(len(pshufd_forms), 2)
                self.assertEqual(
                    sum(row["profile_match_frequencies"].get("S2-35.35-PSHUFD-XMM-register-register", 0) for row in pshufd_forms),
                    805,
                )
                self.assertEqual(
                    sum(row["classification_frequencies"].get("proven_supported", 0) for row in pshufd_forms),
                    805,
                )
                pshufd_memory = [
                    row for row in compact["normalized_instruction_forms"]
                    if row["instruction_form"]["operand_signature"] == "pshufd xmm,mem128,imm"
                ]
                self.assertEqual(len(pshufd_memory), 1)
                self.assertEqual(pshufd_memory[0]["classification_frequencies"], {"unclassified": 1})
                psrldq_forms = [
                    row for row in compact["normalized_instruction_forms"]
                    if row["instruction_form"]["opcode_bytes"] == ["0F", "73"]
                    and row["instruction_form"]["operand_signature"] == "psrldq xmm,imm"
                ]
                self.assertEqual(len(psrldq_forms), 2)
                self.assertEqual(sum(row["count"] for row in psrldq_forms), 355)
                self.assertEqual(
                    sum(row["profile_match_frequencies"].get("S2-35.47-PSRLDQ-XMM-immediate-register", 0) for row in psrldq_forms),
                    355,
                )
                self.assertEqual(
                    sum(row["classification_frequencies"].get("proven_supported", 0) for row in psrldq_forms),
                    355,
                )
                pslld_forms = [
                    row for row in compact["normalized_instruction_forms"]
                    if row["instruction_form"]["opcode_bytes"] == ["0F", "72"]
                    and row["instruction_form"]["operand_signature"] == "pslld xmm,imm"
                ]
                self.assertEqual(len(pslld_forms), 1)
                self.assertEqual(sum(row["count"] for row in pslld_forms), 2)
                self.assertEqual(
                    sum(row["profile_match_frequencies"].get("S2-35.41-PSLLD-XMM-immediate-register", 0) for row in pslld_forms),
                    2,
                )
                self.assertEqual(
                    sum(row["classification_frequencies"].get("proven_supported", 0) for row in pslld_forms),
                    2,
                )
                psrlq_forms = [
                    row for row in compact["normalized_instruction_forms"]
                    if row["instruction_form"]["opcode_bytes"] == ["0F", "73"]
                    and row["instruction_form"]["operand_signature"] == "psrlq xmm,imm"
                ]
                self.assertEqual(len(psrlq_forms), 1)
                self.assertEqual(sum(row["count"] for row in psrlq_forms), 5)
                self.assertEqual(
                    sum(row["profile_match_frequencies"].get("S2-35.45-PSRLQ-XMM-immediate-register", 0) for row in psrlq_forms),
                    5,
                )
                self.assertEqual(
                    sum(row["classification_frequencies"].get("proven_supported", 0) for row in psrlq_forms),
                    5,
                )
                self.assertEqual(
                    compact_fields.get("sha256", compact_fields.get("compact_inventory_sha256")),
                    hashlib.sha256(compact_path.read_bytes()).hexdigest(),
                )
                raw_sha = fields["raw_inventory_sha256"]
                self.assertEqual(len(raw_sha), 64)
                self.assertTrue(all(character in "0123456789abcdef" for character in raw_sha))
                if full_key:
                    self.assertEqual(raw_sha, manifest["runs"][0]["sha256"])

    def test_normalizes_register_encoding_but_preserves_semantic_prefix_and_address(self) -> None:
        plain_a = exporter.normalized_instruction_form({
            "prefix_bytes": ["48"], "opcode_bytes": ["8B"],
            "modrm": {"raw": "00", "mod": 0, "reg": 0, "rm": 0},
            "operand_signature": "mov r64,mem64",
            "address": {"form": "base_only", "size": 64, "segment": None},
        })
        plain_b = exporter.normalized_instruction_form({
            "prefix_bytes": ["48"], "opcode_bytes": ["8B"],
            "modrm": {"raw": "03", "mod": 0, "reg": 0, "rm": 3},
            "operand_signature": "mov r64,mem64",
            "address": {"form": "base_only", "size": 64, "segment": None},
        })
        fs = dict(plain_a, semantic_prefixes=["fs", "rex_w"], address={"form": "base_only", "size": 64, "segment": "fs"})
        self.assertEqual(plain_a, plain_b)
        self.assertNotEqual(plain_a, fs)

    def test_preserves_fs_gs_lock_width_and_addressing_but_not_literal_values(self) -> None:
        base = {
            "prefix_bytes": ["48"], "opcode_bytes": ["83"],
            "modrm": {"raw": "00", "mod": 0, "reg": 0, "rm": 0},
            "operand_signature": "add mem64,imm",
            "address": {"form": "base_only", "size": 64, "segment": None},
        }
        def form(prefix, address, signature=None):
            return exporter.normalized_instruction_form({
                **base, "prefix_bytes": prefix, "address": address,
                "operand_signature": signature or base["operand_signature"],
            })
        ordinary = form(["48"], {"form": "base_only", "size": 64, "segment": None})
        self.assertNotEqual(ordinary, form(["64", "48"], {"form": "base_only", "size": 64, "segment": "fs"}))
        self.assertNotEqual(ordinary, form(["65", "48"], {"form": "base_only", "size": 64, "segment": "gs"}))
        self.assertNotEqual(ordinary, form(["F0", "48"], {"form": "base_only", "size": 64, "segment": None}))
        self.assertNotEqual(ordinary, form(["66", "48"], {"form": "base_only", "size": 64, "segment": None}, "add mem16,imm"))
        self.assertNotEqual(ordinary, form(["48"], {"form": "scaled_index", "size": 64, "segment": None}))
        # The input has no literal immediate/displacement bytes; equivalent
        # register-number or literal-value changes therefore share one form.
        self.assertEqual(ordinary, form(["48"], {"form": "base_only", "size": 64, "segment": None}))

        add_one = analyzer.parse_row("  401000: 48 83 c0 01 add rax,0x1")
        add_seven = analyzer.parse_row("  401004: 48 83 c0 07 add rax,0x7")
        load_one = analyzer.parse_row("  401008: 48 8b 40 01 mov rax,QWORD PTR [rax+0x1]")
        load_four = analyzer.parse_row("  40100c: 48 8b 40 04 mov rax,QWORD PTR [rax+0x4]")
        self.assertEqual(
            exporter.normalized_instruction_form(analyzer.instruction_dimensions(add_one)),
            exporter.normalized_instruction_form(analyzer.instruction_dimensions(add_seven)),
        )
        self.assertEqual(
            exporter.normalized_instruction_form(analyzer.instruction_dimensions(load_one)),
            exporter.normalized_instruction_form(analyzer.instruction_dimensions(load_four)),
        )

    def test_normalizes_register_embedded_opcode_bits_but_keeps_exact_keys(self) -> None:
        push_rax = analyzer.parse_row("  401000: 50 push rax")
        push_rcx = analyzer.parse_row("  401001: 51 push rcx")
        mov_rax = analyzer.parse_row("  401002: 48 b8 01 00 00 00 00 00 00 00 movabs rax,0x1")
        mov_rcx = analyzer.parse_row("  40100c: 48 b9 01 00 00 00 00 00 00 00 movabs rcx,0x1")
        push_a_dims = analyzer.instruction_dimensions(push_rax)
        push_b_dims = analyzer.instruction_dimensions(push_rcx)
        mov_a_dims = analyzer.instruction_dimensions(mov_rax)
        mov_b_dims = analyzer.instruction_dimensions(mov_rcx)
        self.assertNotEqual(push_a_dims["encoding_key"], push_b_dims["encoding_key"])
        self.assertNotEqual(mov_a_dims["encoding_key"], mov_b_dims["encoding_key"])
        self.assertEqual(exporter.normalized_instruction_form(push_a_dims), exporter.normalized_instruction_form(push_b_dims))
        self.assertEqual(exporter.normalized_instruction_form(mov_a_dims), exporter.normalized_instruction_form(mov_b_dims))

        bswap_eax = analyzer.parse_row("  401010: 0f c8 bswap eax")
        bswap_edi = analyzer.parse_row("  401012: 0f cf bswap edi")
        bswap_a = analyzer.instruction_dimensions(bswap_eax)
        bswap_b = analyzer.instruction_dimensions(bswap_edi)
        self.assertNotEqual(bswap_a["encoding_key"], bswap_b["encoding_key"])
        self.assertEqual(exporter.normalized_instruction_form(bswap_a), exporter.normalized_instruction_form(bswap_b))

    def test_aggregates_exact_variants_and_unclassified_counts_per_binary(self) -> None:
        supported = exact_form(["48"], "00", {"form": "base_only", "size": 64, "segment": None}, 3, "bash", "proven_supported")
        unknown = exact_form(["48"], "03", {"form": "base_only", "size": 64, "segment": None}, 2, "bash", "unclassified")
        report = {
            "source_image": {"sha256": "image-hash"},
            "disassembler": {}, "profile_comparison": {},
            "inventory": {
                "encoding_forms": [supported, unknown],
                "instruction_count": 5, "classification_frequencies": {"proven_supported": 3, "unclassified": 2},
                "evidence_scope_frequencies": {"local_interpreter_profile": 3},
                "conditional_cpl0_candidate_frequencies": {},
            },
            "conditional_candidates": {"supported_overlap_rows": 0},
            "binaries": [{
                "name": "bash", "image_path": "/usr/bin/bash", "size_bytes": 100,
                "sha256": "elf-hash", "elf_machine": "Advanced Micro Devices X86-64",
                "build_id": "build-id", "instruction_count": 5,
                "classification_frequencies": {"proven_supported": 3, "unclassified": 2},
            }],
        }
        result = exporter.aggregate(report)
        self.assertEqual(result["normalized_instruction_form_count"], 1)
        self.assertEqual(result["normalized_instruction_forms"][0]["count"], 5)
        self.assertEqual(result["binaries"]["bash"]["top_unclassified_forms"][0]["count"], 2)
        self.assertEqual(result["scenarios"]["C"]["unclassified_instruction_rows"], 2)
        self.assertEqual(result["scenarios"]["D"]["unclassified_instruction_rows"], 2)
        self.assertEqual(result["scenarios"]["C"]["conditional_candidate_instruction_rows"], 0)
        self.assertEqual(result["exact_instruction_rows_with_any_conditional_candidate"], 0)
        self.assertEqual(result["conditional_candidate_supported_overlap_rows"], 0)
        self.assertIn("bash", result["scenarios"]["C"]["binaries"])
        self.assertIn("bash", result["scenarios"]["D"]["binaries"])

    def test_e_static_inventory_is_separate_from_runtime_generated_code(self) -> None:
        exact = exact_form(["48"], "00", {"form": "base_only", "size": 64, "segment": None}, 7, "stage5-e-static", "unclassified")
        inventory = {
            "source_image": {"sha256": "accepted-rootfs"},
            "disassembler": {},
            "profile_comparison": {"registry_complete": False},
            "external_inputs": [{
                "name": "stage5-e-static",
                "path": "work/stage5/s5-21/stage5-e-static.elf",
                "sha256": "static-elf-hash",
                "source_c_sha256": "source-hash",
            }],
            "inventory": {
                "encoding_forms": [exact],
                "instruction_count": 7,
                "classification_frequencies": {"unclassified": 7},
                "evidence_scope_frequencies": {},
                "conditional_cpl0_candidate_frequencies": {},
            },
            "conditional_candidates": {"supported_overlap_rows": 0},
            "binaries": [{
                "name": "stage5-e-static", "image_path": "external:stage5-e-static.elf", "size_bytes": 100,
                "sha256": "static-elf-hash", "elf_machine": "Advanced Micro Devices X86-64",
                "build_id": "", "instruction_count": 7,
                "classification_frequencies": {"unclassified": 7},
            }],
        }
        result = exporter.aggregate(inventory)
        e = result["scenarios"]["E"]
        self.assertEqual(e["status"], "static_executable_only")
        self.assertEqual(e["static_executable"]["sha256"], "static-elf-hash")
        self.assertEqual(e["static_executable"]["unclassified_instruction_rows"], 7)
        self.assertEqual(e["runtime_generated_code"]["status"], "not_inferred")
        self.assertEqual(e["runtime_generated_code"]["frequency"], "not_inferred")
        self.assertNotIn("stage5-e-static", result["scenarios"]["C"]["binaries"])
        self.assertNotIn("stage5-e-static", result["scenarios"]["D"]["binaries"])


if __name__ == "__main__":
    unittest.main()
