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
BASE_COMMIT = "63581fb3ae8e31b2cdb4f64d70c10deb4165253f"
INVENTORIES = {
    "docs/reports/data/stage5/s5-21-isa-inventory.json.gz": (
        "b9059f45adda311f3a11950c440d75e67c22d255c7d2899d13b76c7dc3d12377",
        "beb92e83d2c02c17dd50a9a616aad298a79bf42ca9f7db32cdf2b970f1f1eec4",
    ),
    "docs/reports/data/stage5/s5-21-isa-inventory-e-static.json.gz": (
        "d266793df5c0f924cd77f76b57f9c7898bca721be5c41989de8395ed5039084c",
        "2d332123379c6bf55a065ebbcb317fbe77a6dc4c111444337df05da118c71066",
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


if __name__ == "__main__":
    unittest.main()
