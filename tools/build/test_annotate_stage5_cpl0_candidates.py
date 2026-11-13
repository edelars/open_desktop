from __future__ import annotations

import gzip
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import annotate_stage5_cpl0_candidates as annotate


ROOT = Path(__file__).resolve().parents[2]
BASE_COMMIT = "45401a1fcb5487fa83b12b3c26abda4dd137711e"
INVENTORIES = {
    "docs/reports/data/stage5/s5-21-isa-inventory.json.gz": (
        "7dceca57748810beb6d1c05802c9d1a83aeb6e2c61faa0d36d3eab6b1864b3a9",
        "5b688b74a78417bab072e2e4c3f749cb187feb7aa6fc259f3ae07edb118079f2",
    ),
    "docs/reports/data/stage5/s5-21-isa-inventory-e-static.json.gz": (
        "a4032b62e963025f4e0020306968807cd306e91e56ba15a9b285a51b08cb47ad",
        "40dae558902d1713b6aef259cf99d7646957c13a1c7e8ca3e38c4012a727c239",
    ),
}


class PrefetchCandidateAnnotationTests(unittest.TestCase):
    def _baseline_payload(self, relative: str) -> dict:
        baseline = subprocess.run(
            ["git", "show", f"{BASE_COMMIT}:{relative}"],
            cwd=ROOT,
            check=True,
            stdout=subprocess.PIPE,
        ).stdout
        self.assertEqual(hashlib.sha256(baseline).hexdigest(), INVENTORIES[relative][0])
        return json.loads(gzip.decompress(baseline))

    def _assert_mutation_is_rejected(self, exact_mutation, normalized_mutation) -> None:
        data = self._baseline_payload("docs/reports/data/stage5/s5-21-isa-inventory.json.gz")
        candidate = "S4-38L6a-PREFETCHNTA-base-only"
        exact = next(
            record for record in data["exact_encoding_forms"]
            if record["dimensions"]["operand_signature"] == "prefetchnta mem8"
        )
        normalized = next(
            record for record in data["normalized_instruction_forms"]
            if record["instruction_form"]["operand_signature"] == "prefetchnta mem8"
        )
        exact["conditional_candidate_frequencies"].pop(candidate, None)
        normalized["conditional_candidate_frequencies"].pop(candidate, None)
        exact_mutation(exact["dimensions"])
        normalized_mutation(normalized["instruction_form"])
        self.assertNotIn(candidate, exact["conditional_candidate_frequencies"])
        self.assertNotIn(candidate, normalized["conditional_candidate_frequencies"])

        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / Path("docs/reports/data/stage5/s5-21-isa-inventory.json.gz").name
            mutated = gzip.compress(json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode(), mtime=0)
            target.write_bytes(mutated)
            with self.assertRaisesRegex(ValueError, "observed candidate counts changed"):
                annotate.annotate(target)
            # The strict observed-count guard rejects the mutated inventory before writing.
            self.assertEqual(target.read_bytes(), mutated)
            after = json.loads(gzip.decompress(target.read_bytes()))
            after_exact = next(
                record for record in after["exact_encoding_forms"]
                if record["dimensions"]["operand_signature"] == "prefetchnta mem8"
            )
            after_normalized = next(
                record for record in after["normalized_instruction_forms"]
                if record["instruction_form"]["operand_signature"] == "prefetchnta mem8"
            )
            self.assertNotIn(candidate, after_exact["conditional_candidate_frequencies"])
            self.assertNotIn(candidate, after_normalized["conditional_candidate_frequencies"])

    def test_prefetch_mismatched_encoding_mutations_are_rejected(self) -> None:
        mutations = {
            "extra_prefix": (
                lambda d: d.update(prefix_bytes=["66"]),
                lambda f: f.update(semantic_prefixes=["operand_size"]),
            ),
            "reordered_prefix": (
                lambda d: d.update(prefix_bytes=["F3", "66"]),
                lambda f: f.update(semantic_prefixes=["rep", "operand_size"]),
            ),
            "wrong_opcode": (
                lambda d: d.update(opcode_bytes=["0F", "19"]),
                lambda f: f.update(opcode_bytes=["0F", "19"]),
            ),
            "wrong_length": (
                lambda d: d.update(instruction_length=4),
                lambda f: None,
            ),
            "modrm_mode": (
                lambda d: d["modrm"].update(mod=3, raw="C0"),
                lambda f: f.update(modrm_mode="register"),
            ),
            "modrm_reg": (
                lambda d: d["modrm"].update(reg=1, raw="08"),
                lambda f: f.update(group_extension=1),
            ),
            "modrm_rm": (
                lambda d: d["modrm"].update(rm=1, raw="01"),
                lambda f: None,
            ),
            "wrong_address": (
                lambda d: d.update(address={"form": "rip_relative", "segment": None, "size": 64}),
                lambda f: f.update(address={"form": "rip_relative", "segment": None, "size": 64}),
            ),
            "sib_present": (
                lambda d: d.update(sib={"base": 0, "index": 1, "scale": 1}),
                lambda f: f.update(address={"form": "sib", "segment": None, "size": 64}),
            ),
            "segment_override": (
                lambda d: d.update(address={"form": "base_only", "segment": "FS", "size": 64}),
                lambda f: f.update(address={"form": "base_only", "segment": "FS", "size": 64}),
            ),
        }
        for name, (exact_mutation, normalized_mutation) in mutations.items():
            with self.subTest(mutation=name):
                self._assert_mutation_is_rejected(exact_mutation, normalized_mutation)

    def test_exact_prefetch_forms_are_annotated_and_derivation_repeats(self) -> None:
        first_by_inventory = {}
        for relative, (expected_base_sha, expected_final_sha) in INVENTORIES.items():
            with tempfile.TemporaryDirectory() as tmp:
                target = Path(tmp) / Path(relative).name
                baseline = subprocess.run(
                    ["git", "show", f"{BASE_COMMIT}:{relative}"],
                    cwd=ROOT,
                    check=True,
                    stdout=subprocess.PIPE,
                ).stdout
                self.assertEqual(hashlib.sha256(baseline).hexdigest(), expected_base_sha)
                target.write_bytes(baseline)
                annotate.annotate(target)
                first = target.read_bytes()
                annotate.annotate(target)
                second = target.read_bytes()
                self.assertEqual(first, second)
                self.assertEqual(hashlib.sha256(first).hexdigest(), expected_final_sha)
                first_by_inventory[relative] = first

        first = first_by_inventory["docs/reports/data/stage5/s5-21-isa-inventory.json.gz"]
        data = json.loads(gzip.decompress(first))
        expected = {
            "S4-38L6a-PREFETCHNTA-base-only": ("prefetchnta mem8", 0, "00"),
            "S4-38L6a-PREFETCHT0-base-only": ("prefetcht0 mem8", 1, "08"),
        }
        found = {candidate: [] for candidate in expected}
        for record in data["exact_encoding_forms"]:
            dims = record["dimensions"]
            for candidate, (operand, reg, raw) in expected.items():
                if candidate in record["conditional_candidate_frequencies"]:
                    self.assertEqual(record["conditional_candidate_frequencies"][candidate], record["count"])
                    self.assertEqual(record["classification_frequencies"], {"unclassified": record["count"]})
                    self.assertEqual(record["evidence_scope_frequencies"], {})
                    self.assertEqual(dims["operand_signature"], operand)
                    self.assertEqual(dims["opcode_bytes"], ["0F", "18"])
                    self.assertEqual(dims["prefix_bytes"], [])
                    self.assertEqual(dims["instruction_length"], 3)
                    self.assertEqual(dims["modrm"], {"mod": 0, "raw": raw, "reg": reg, "rm": 0})
                    self.assertEqual(dims["address"], {"form": "base_only", "segment": None, "size": 64})
                    self.assertIsNone(dims["sib"])
                    found[candidate].append(record["count"])
                elif dims["opcode_bytes"] == ["0F", "18"]:
                    self.assertNotIn(candidate, record["conditional_candidate_frequencies"])
        self.assertEqual(found, {key: [3] for key in expected})
        for record in data["normalized_instruction_forms"]:
            form = record["instruction_form"]
            for candidate, (operand, _, _) in expected.items():
                if form["opcode_bytes"] == ["0F", "18"] and form["operand_signature"] == operand:
                    is_exact_candidate_shape = (
                        form["address"] == {"form": "base_only", "segment": None, "size": 64}
                        and form["modrm_mode"] == "memory"
                        and form["group_extension"] is None
                        and form["semantic_prefixes"] == []
                    )
                    self.assertEqual(candidate in record["conditional_candidate_frequencies"], is_exact_candidate_shape)

    def test_rep_bsf_register_gate_rejects_mismatched_forms(self) -> None:
        mutations = {
            "extra_prefix": (
                lambda d: (d.update(prefix_bytes=["F3", "66", "48"]), d.update(instruction_length=6)),
                lambda f: f.update(semantic_prefixes=["rep", "operand_size", "rex_w"]),
            ),
            "reordered_prefix": (
                lambda d: (d.update(prefix_bytes=["48", "F3"]), d.update(instruction_length=5)),
                lambda f: f.update(semantic_prefixes=["rex_w", "rep"]),
            ),
            "wrong_opcode": (
                lambda d: d.update(opcode_bytes=["0F", "BD"]),
                lambda f: f.update(opcode_bytes=["0F", "BD"]),
            ),
            "wrong_length": (
                lambda d: d.update(instruction_length=6),
                lambda f: None,
            ),
            "non_register_modrm": (
                lambda d: (d["modrm"].update(mod=0, raw="00"), d.update(address={"form": "base_only", "segment": None, "size": 64}, sib=None, operand_signature="tzcnt r64,mem64")),
                lambda f: (f.update(modrm_mode="memory", address={"form": "base_only", "segment": None, "size": 64}, operand_signature="tzcnt r64,mem64")),
            ),
            "bad_rex_byte": (
                lambda d: d.update(prefix_bytes=["F3", "50"]),
                lambda f: f.update(semantic_prefixes=["rep", "rex"]),
            ),
            "wrong_address": (
                lambda d: d.update(address={"form": "rip_relative", "segment": None, "size": 64}),
                lambda f: f.update(address={"form": "rip_relative", "segment": None, "size": 64}),
            ),
            "segment_override": (
                lambda d: d.update(address={"form": "base_only", "segment": "FS", "size": 64}),
                lambda f: f.update(address={"form": "base_only", "segment": "FS", "size": 64}),
            ),
        }
        candidate = "S4-38L4g-REP-BSF-register-BMI1-clear"
        for name, (exact_mutation, normalized_mutation) in mutations.items():
            with self.subTest(mutation=name):
                data = self._baseline_payload("docs/reports/data/stage5/s5-21-isa-inventory.json.gz")
                exact = next(
                    record for record in data["exact_encoding_forms"]
                    if record["dimensions"]["operand_signature"] == "tzcnt r64,r64"
                )
                normalized = next(
                    record for record in data["normalized_instruction_forms"]
                    if record["instruction_form"]["operand_signature"] == "tzcnt r64,r64"
                )
                exact["conditional_candidate_frequencies"].pop(candidate, None)
                normalized["conditional_candidate_frequencies"].pop(candidate, None)
                exact_mutation(exact["dimensions"])
                normalized_mutation(normalized["instruction_form"])
                self.assertNotIn(candidate, exact["conditional_candidate_frequencies"])
                self.assertNotIn(candidate, normalized["conditional_candidate_frequencies"])
                with tempfile.TemporaryDirectory() as tmp:
                    target = Path(tmp) / "inventory.json.gz"
                    mutated = gzip.compress(json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode(), mtime=0)
                    target.write_bytes(mutated)
                    with self.assertRaisesRegex(ValueError, "observed candidate counts changed"):
                        annotate.annotate(target)
                    self.assertEqual(target.read_bytes(), mutated)

    def test_rep_bsf_register_forms_keep_counts_and_exclude_memory(self) -> None:
        candidate = "S4-38L4g-REP-BSF-register-BMI1-clear"
        expected = {
            "docs/reports/data/stage5/s5-21-isa-inventory.json.gz": {
                "cpl0": 115683, "all": 116191, "unique": 116114,
                "classification": {"proven_supported": 6296422, "unclassified": 213978},
            },
            "docs/reports/data/stage5/s5-21-isa-inventory-e-static.json.gz": {
                "cpl0": 115743, "all": 116251, "unique": 116174,
                "classification": {"proven_supported": 6296706, "unclassified": 214039},
            },
        }
        for relative, want in expected.items():
            baseline = subprocess.run(
                ["git", "show", f"{BASE_COMMIT}:{relative}"], cwd=ROOT,
                check=True, stdout=subprocess.PIPE,
            ).stdout
            with tempfile.TemporaryDirectory() as tmp:
                target = Path(tmp) / "inventory.json.gz"
                target.write_bytes(baseline)
                annotate.annotate(target)
                data = json.loads(gzip.decompress(target.read_bytes()))
            self.assertEqual(data["conditional_cpl0_candidate_hit_count"], want["cpl0"])
            self.assertEqual(data["conditional_candidate_hit_count"], want["all"])
            self.assertEqual(data["exact_instruction_rows_with_any_conditional_candidate"], want["unique"])
            self.assertEqual(data["conditional_candidate_supported_overlap_rows"], 40)
            self.assertEqual(data["classification_frequencies"], want["classification"])
            self.assertEqual(data["conditional_cpl0_candidate_frequencies"][candidate], 398)
            self.assertEqual(data["binaries"]["gcc-cc1"]["conditional_cpl0_candidate_frequencies"][candidate], 391)
            self.assertEqual(data["binaries"]["gcc-as"]["conditional_cpl0_candidate_frequencies"][candidate], 5)
            self.assertEqual(data["binaries"]["gcc-driver"]["conditional_cpl0_candidate_frequencies"][candidate], 1)
            self.assertEqual(data["binaries"]["gcc-ld-bfd"]["conditional_cpl0_candidate_frequencies"][candidate], 1)
            exact_matches = [
                record for record in data["exact_encoding_forms"]
                if candidate in record["conditional_candidate_frequencies"]
            ]
            self.assertEqual(len(exact_matches), 72)
            self.assertEqual(sum(record["count"] for record in exact_matches), 398)
            self.assertTrue(all(record["dimensions"]["modrm"]["mod"] == 3 for record in exact_matches))
            self.assertTrue(all(record["classification_frequencies"] == {"unclassified": record["count"]} for record in exact_matches))
            self.assertTrue(all(record["evidence_scope_frequencies"] == {} for record in exact_matches))
            memory_forms = [
                record for record in data["exact_encoding_forms"]
                if record["dimensions"]["opcode_bytes"] == ["0F", "BC"]
                and "F3" in record["dimensions"]["prefix_bytes"]
                and record["dimensions"]["modrm"]["mod"] != 3
            ]
            self.assertEqual(sum(record["count"] for record in memory_forms), 2)
            self.assertTrue(all(candidate not in record["conditional_candidate_frequencies"] for record in memory_forms))
            for record in exact_matches:
                dims = record["dimensions"]
                signature = dims["operand_signature"]
                prefixes = dims["prefix_bytes"]
                self.assertEqual(prefixes[0], "F3")
                if len(prefixes) == 2:
                    rex = int(prefixes[1], 16)
                    self.assertEqual(bool(rex & 0x08), signature == "tzcnt r64,r64")

    def test_rep_bsf_rex_w_must_match_operand_width(self) -> None:
        data = self._baseline_payload("docs/reports/data/stage5/s5-21-isa-inventory.json.gz")
        candidate = "S4-38L4g-REP-BSF-register-BMI1-clear"
        exact = next(
            record for record in data["exact_encoding_forms"]
            if record["dimensions"]["operand_signature"] == "tzcnt r32,r32"
        )
        normalized = next(
            record for record in data["normalized_instruction_forms"]
            if record["instruction_form"]["operand_signature"] == "tzcnt r32,r32"
            and record["instruction_form"]["semantic_prefixes"] == ["rep"]
        )
        exact["conditional_candidate_frequencies"].pop(candidate, None)
        normalized["conditional_candidate_frequencies"].pop(candidate, None)
        exact["dimensions"]["prefix_bytes"] = ["F3", "49"]
        exact["dimensions"]["instruction_length"] = 5
        normalized["instruction_form"]["semantic_prefixes"] = ["rep", "rex_w"]
        self.assertNotIn(candidate, exact["conditional_candidate_frequencies"])
        self.assertNotIn(candidate, normalized["conditional_candidate_frequencies"])
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "inventory.json.gz"
            mutated = gzip.compress(json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode(), mtime=0)
            target.write_bytes(mutated)
            with self.assertRaisesRegex(ValueError, "observed candidate counts changed"):
                annotate.annotate(target)
            self.assertEqual(target.read_bytes(), mutated)

    def test_rep_bsf_r64_requires_rex_w(self) -> None:
        data = self._baseline_payload("docs/reports/data/stage5/s5-21-isa-inventory.json.gz")
        candidate = "S4-38L4g-REP-BSF-register-BMI1-clear"
        exact = next(
            record for record in data["exact_encoding_forms"]
            if record["dimensions"]["operand_signature"] == "tzcnt r64,r64"
            and record["dimensions"]["prefix_bytes"] == ["F3", "48"]
        )
        exact["conditional_candidate_frequencies"].pop(candidate, None)
        dimensions = exact["dimensions"]
        dimensions["prefix_bytes"] = ["F3"]
        dimensions["instruction_length"] = 4
        self.assertNotIn(candidate, exact["conditional_candidate_frequencies"])
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "inventory.json.gz"
            mutated = gzip.compress(json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode(), mtime=0)
            target.write_bytes(mutated)
            with self.assertRaisesRegex(ValueError, "observed candidate counts changed"):
                annotate.annotate(target)
            self.assertEqual(target.read_bytes(), mutated)

    def test_lock_xadd_candidates_reject_mismatched_encodings(self) -> None:
        mutations = {
            "missing_lock": (
                lambda d: d.update(prefix_bytes=[]),
                lambda f: f.update(semantic_prefixes=[]),
            ),
            "extra_prefix": (
                lambda d: d.update(prefix_bytes=["F0", "66"], instruction_length=6),
                lambda f: f.update(semantic_prefixes=["lock", "operand_size"]),
            ),
            "wrong_opcode": (
                lambda d: d.update(opcode_bytes=["0F", "C0"]),
                lambda f: f.update(opcode_bytes=["0F", "C0"]),
            ),
            "wrong_length": (
                lambda d: d.update(instruction_length=6),
                lambda f: None,
            ),
            "wrong_modrm_mode": (
                lambda d: d["modrm"].update(mod=3, raw="C8"),
                lambda f: f.update(modrm_mode="register"),
            ),
            "unobserved_modrm": (
                lambda d: d["modrm"].update(raw="49"),
                lambda f: None,
            ),
            "modrm_reg_field": (
                lambda d: d["modrm"].update(reg=7),
                lambda f: None,
            ),
            "modrm_rm_field": (
                lambda d: d["modrm"].update(rm=3),
                lambda f: None,
            ),
            "sib_present": (
                lambda d: d.update(sib={"base": 0, "index": 1, "scale": 1}),
                lambda f: f.update(address={"form": "sib", "segment": None, "size": 64}),
            ),
            "wrong_address_size": (
                lambda d: d.update(address={"form": "base_only", "segment": None, "size": 32}),
                lambda f: f.update(address={"form": "base_only", "segment": None, "size": 32}),
            ),
            "segment_override": (
                lambda d: d.update(address={"form": "base_only", "segment": "FS", "size": 64}),
                lambda f: f.update(address={"form": "base_only", "segment": "FS", "size": 64}),
            ),
        }
        candidate = "S4-38L7ay-LOCK-XADD-m32-r32-base-only"
        for name, (exact_mutation, normalized_mutation) in mutations.items():
            with self.subTest(mutation=name):
                data = self._baseline_payload("docs/reports/data/stage5/s5-21-isa-inventory.json.gz")
                exact = next(record for record in data["exact_encoding_forms"]
                             if record["dimensions"]["operand_signature"] == "lock xadd mem32,r32")
                normalized = next(record for record in data["normalized_instruction_forms"]
                                  if record["instruction_form"]["operand_signature"] == "lock xadd mem32,r32")
                exact_mutation(exact["dimensions"])
                normalized_mutation(normalized["instruction_form"])
                self.assertNotIn(candidate, exact["conditional_candidate_frequencies"])
                self.assertNotIn(candidate, normalized["conditional_candidate_frequencies"])
                with tempfile.TemporaryDirectory() as tmp:
                    target = Path(tmp) / "inventory.json.gz"
                    mutated = gzip.compress(json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode(), mtime=0)
                    target.write_bytes(mutated)
                    with self.assertRaisesRegex(ValueError, "observed candidate counts changed"):
                        annotate.annotate(target)
                    self.assertEqual(target.read_bytes(), mutated)

    def test_lock_xadd_candidate_counts_and_classifications(self) -> None:
        candidate = "S4-38L7ay-LOCK-XADD-m32-r32-base-only"
        expected = {
            "docs/reports/data/stage5/s5-21-isa-inventory.json.gz": {
                "cpl0": 115683, "all": 116191, "unique": 116114,
                "classification": {"proven_supported": 6296422, "unclassified": 213978},
            },
            "docs/reports/data/stage5/s5-21-isa-inventory-e-static.json.gz": {
                "cpl0": 115743, "all": 116251, "unique": 116174,
                "classification": {"proven_supported": 6296706, "unclassified": 214039},
            },
        }
        for relative, want in expected.items():
            baseline = subprocess.run(["git", "show", f"{BASE_COMMIT}:{relative}"], cwd=ROOT, check=True, stdout=subprocess.PIPE).stdout
            with tempfile.TemporaryDirectory() as tmp:
                target = Path(tmp) / "inventory.json.gz"
                target.write_bytes(baseline)
                annotate.annotate(target)
                data = json.loads(gzip.decompress(target.read_bytes()))
            self.assertEqual(data["conditional_cpl0_candidate_hit_count"], want["cpl0"])
            self.assertEqual(data["conditional_candidate_hit_count"], want["all"])
            self.assertEqual(data["exact_instruction_rows_with_any_conditional_candidate"], want["unique"])
            self.assertEqual(data["conditional_candidate_supported_overlap_rows"], 40)
            self.assertEqual(data["classification_frequencies"], want["classification"])
            self.assertEqual(data["conditional_cpl0_candidate_frequencies"][candidate], 36)
            self.assertEqual(data["binaries"]["gcc-cc1"]["conditional_cpl0_candidate_frequencies"][candidate], 12)
            self.assertEqual(data["binaries"]["gcc-collect2"]["conditional_cpl0_candidate_frequencies"][candidate], 12)
            self.assertEqual(data["binaries"]["gcc-driver"]["conditional_cpl0_candidate_frequencies"][candidate], 12)
            self.assertTrue(all(record["classification_frequencies"] == {"unclassified": record["count"]}
                                and not record["evidence_scope_frequencies"]
                                for record in data["exact_encoding_forms"]
                                if candidate in record["conditional_candidate_frequencies"]))
            matches = [record for record in data["exact_encoding_forms"]
                       if candidate in record["conditional_candidate_frequencies"]]
            self.assertEqual(len(matches), 6)
            self.assertEqual(sum(record["count"] for record in matches), 36)

    def test_scenario_candidate_summaries_match_exact_records(self) -> None:
        candidate = "S4-38L7ay-LOCK-XADD-m32-r32-base-only"
        for relative in INVENTORIES:
            baseline = subprocess.run(["git", "show", f"{BASE_COMMIT}:{relative}"], cwd=ROOT, check=True, stdout=subprocess.PIPE).stdout
            with tempfile.TemporaryDirectory() as tmp:
                target = Path(tmp) / "inventory.json.gz"
                target.write_bytes(baseline)
                annotate.annotate(target)
                data = json.loads(gzip.decompress(target.read_bytes()))
            exact = data["exact_encoding_forms"]
            all_cpl0 = {key for record in exact for key in record["conditional_candidate_frequencies"]}
            all_local = {key for record in exact for key in record.get("conditional_local_candidate_frequencies", {})}
            for scenario_name in ("C", "D"):
                scenario = data["scenarios"][scenario_name]
                binaries = scenario["binaries"]
                cpl0 = {key: 0 for key in all_cpl0}
                local = {key: 0 for key in all_local}
                instruction_rows = 0
                for record in exact:
                    binary_rows = sum(record["binary_frequencies"].get(binary, 0) for binary in binaries)
                    cpl0_candidates = record["conditional_candidate_frequencies"]
                    local_candidates = record.get("conditional_local_candidate_frequencies", {})
                    if cpl0_candidates or local_candidates:
                        instruction_rows += binary_rows
                    for key in cpl0_candidates:
                        cpl0[key] += binary_rows
                    for key in local_candidates:
                        local[key] += binary_rows
                self.assertEqual(scenario["conditional_candidate_frequencies"], cpl0)
                self.assertEqual(scenario["conditional_local_candidate_frequencies"], local)
                self.assertEqual(scenario["conditional_candidate_instruction_rows"], instruction_rows)
            self.assertEqual(data["scenarios"]["C"]["conditional_candidate_frequencies"][candidate], 0)
            self.assertEqual(data["scenarios"]["D"]["conditional_candidate_frequencies"][candidate], 36)
            self.assertEqual(data["scenarios"]["C"]["conditional_candidate_frequencies"]["S4-38L6d-LEAVE-bare-C9"], 1054)


if __name__ == "__main__":
    unittest.main()
