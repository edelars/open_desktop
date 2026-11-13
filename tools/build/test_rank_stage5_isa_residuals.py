from __future__ import annotations

import gzip
import json
import tempfile
import unittest
from pathlib import Path

import rank_stage5_isa_residuals as ranker


class RankedResidualTests(unittest.TestCase):
    def test_current_inventory_totals_and_rankings(self) -> None:
        report = ranker.build_report()
        self.assertEqual(report["rootfs"]["totals"], {"rows": 97808, "normalized_forms": 157, "exact_keys": 1973})
        self.assertEqual(report["e_inclusive"]["totals"], {"rows": 97809, "normalized_forms": 157, "exact_keys": 1973})
        self.assertEqual(report["e_delta"]["rows"], 1)
        self.assertEqual(report["e_delta"]["normalized_forms"], 0)
        self.assertEqual(report["e_delta"]["exact_keys"], 0)
        self.assertEqual(
            (report["rootfs"]["scenarios"]["C"]["rows"], report["rootfs"]["scenarios"]["C"]["normalized_forms"], report["rootfs"]["scenarios"]["C"]["exact_keys"]),
            (2213, 61, 322),
        )
        self.assertEqual(
            (report["rootfs"]["scenarios"]["D"]["rows"], report["rootfs"]["scenarios"]["D"]["normalized_forms"], report["rootfs"]["scenarios"]["D"]["exact_keys"]),
            (97152, 149, 1937),
        )
        self.assertEqual(report["rootfs"]["scenarios"]["C"]["top_normalized_forms"][0]["rows"], 534)
        self.assertEqual(report["rootfs"]["scenarios"]["D"]["top_normalized_forms"][0]["rows"], 23789)

    def test_pair_generation_is_deterministic(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outputs = [(root / f"rank-{i}.json", root / f"rank-{i}.md") for i in range(2)]
            for json_path, markdown_path in outputs:
                ranker.write_outputs(ranker.ROOTFS_DEFAULT, ranker.E_DEFAULT, json_path, markdown_path)
            self.assertEqual(outputs[0][0].read_bytes(), outputs[1][0].read_bytes())
            self.assertEqual(outputs[0][1].read_bytes(), outputs[1][1].read_bytes())
            markdown = outputs[0][1].read_text(encoding="utf-8")
            self.assertIn("does not estimate runtime frequency, speedup, implementation cost", markdown)
            self.assertIn("## Scenario C", markdown)
            self.assertIn("## Scenario D", markdown)

    def test_partial_exact_profile_excludes_whole_normalized_form(self) -> None:
        dimensions = {
            "encoding_key": "synthetic-op-a",
            "operand_signature": "synthetic operation",
            "prefix_bytes": [],
            "opcode_bytes": ["90"],
            "instruction_length": 1,
            "modrm": None,
            "address": {"form": "none", "segment": None, "size": None},
            "sib": None,
        }
        normalized = ranker.normalized_instruction_form(dimensions)

        def exact(key: str, *, candidate: bool = False) -> dict:
            item_dimensions = {**dimensions, "encoding_key": key}
            return {
                "dimensions": item_dimensions,
                "count": 1,
                "binary_frequencies": {"bash": 1},
                "classification_frequencies": {"unclassified": 1},
                "evidence_scope_frequencies": {},
                "profile_match_frequencies": {},
                "conditional_candidate_frequencies": {"accepted-profile": 1} if candidate else {},
                "conditional_local_candidate_frequencies": {},
            }

        partial_keys = [exact("synthetic-op-a"), exact("synthetic-op-b", candidate=True)]
        clean_key = {
            **exact("synthetic-op-clean"),
            "dimensions": {**dimensions, "encoding_key": "synthetic-op-clean", "opcode_bytes": ["0F", "0B"]},
        }
        clean_normalized = ranker.normalized_instruction_form(clean_key["dimensions"])
        compact = {
            "normalized_instruction_forms": [
                {
                    "instruction_form": normalized,
                    "count": 2,
                    "binary_frequencies": {"bash": 2},
                    "classification_frequencies": {"unclassified": 2},
                    "evidence_scope_frequencies": {},
                    "profile_match_frequencies": {},
                    "conditional_candidate_frequencies": {},
                    "conditional_local_candidate_frequencies": {},
                },
                {
                    "instruction_form": clean_normalized,
                    "count": 1,
                    "binary_frequencies": {"bash": 1},
                    "classification_frequencies": {"unclassified": 1},
                    "evidence_scope_frequencies": {},
                    "profile_match_frequencies": {},
                    "conditional_candidate_frequencies": {},
                    "conditional_local_candidate_frequencies": {},
                },
            ],
            "exact_encoding_forms": partial_keys + [clean_key],
            "scenarios": {"C": {"binaries": ["bash"]}, "D": {"binaries": ["bash"]}},
        }
        ranked = ranker.rank_inventory(compact, "synthetic-sha", source_label="synthetic")
        self.assertEqual(ranked["totals"], {"rows": 1, "normalized_forms": 1, "exact_keys": 1})
        self.assertEqual(ranked["scenarios"]["C"]["rows"], 1)
        self.assertEqual(ranked["scenarios"]["C"]["exact_keys"], 1)
        self.assertEqual(ranked["scenarios"]["C"]["top_normalized_forms"][0]["opcode_bytes"], ["0F", "0B"])

    def test_all_annotation_maps_exclude_at_aggregate_and_exact_level(self) -> None:
        dimensions = {
            "encoding_key": "synthetic-op",
            "operand_signature": "synthetic operation",
            "prefix_bytes": [],
            "opcode_bytes": ["0F", "0B"],
            "instruction_length": 2,
            "modrm": None,
            "address": {"form": "none", "segment": None, "size": None},
            "sib": None,
        }
        annotation_maps = (
            "conditional_candidate_frequencies",
            "conditional_local_candidate_frequencies",
            "evidence_scope_frequencies",
            "profile_match_frequencies",
        )

        def record(key: str, **annotations: dict) -> dict:
            return {
                "dimensions": {**dimensions, "encoding_key": key},
                "count": 1,
                "binary_frequencies": {"bash": 1},
                "classification_frequencies": {"unclassified": 1},
                "evidence_scope_frequencies": {},
                "profile_match_frequencies": {},
                "conditional_candidate_frequencies": {},
                "conditional_local_candidate_frequencies": {},
                **annotations,
            }

        normalized = ranker.normalized_instruction_form(dimensions)
        for annotation_map in annotation_maps:
            with self.subTest(level="exact", annotation_map=annotation_map):
                exact_records = [record("clean"), record("annotated", **{annotation_map: {"match": 1}})]
                compact = {
                    "normalized_instruction_forms": [
                        {
                            "instruction_form": normalized,
                            "count": 2,
                            "binary_frequencies": {"bash": 2},
                            "classification_frequencies": {"unclassified": 2},
                            "evidence_scope_frequencies": {},
                            "profile_match_frequencies": {},
                            "conditional_candidate_frequencies": {},
                            "conditional_local_candidate_frequencies": {},
                        }
                    ],
                    "exact_encoding_forms": exact_records,
                    "scenarios": {"C": {"binaries": ["bash"]}, "D": {"binaries": ["bash"]}},
                }
                self.assertEqual(ranker.rank_inventory(compact, "sha", source_label="test")["totals"]["rows"], 0)

            with self.subTest(level="aggregate", annotation_map=annotation_map):
                clean = record("clean")
                aggregate = {
                    "instruction_form": normalized,
                    "count": 1,
                    "binary_frequencies": {"bash": 1},
                    "classification_frequencies": {"unclassified": 1},
                    "evidence_scope_frequencies": {},
                    "profile_match_frequencies": {},
                    "conditional_candidate_frequencies": {},
                    "conditional_local_candidate_frequencies": {},
                    annotation_map: {"match": 1},
                }
                compact = {
                    "normalized_instruction_forms": [aggregate],
                    "exact_encoding_forms": [clean],
                    "scenarios": {"C": {"binaries": ["bash"]}, "D": {"binaries": ["bash"]}},
                }
                self.assertEqual(ranker.rank_inventory(compact, "sha", source_label="test")["totals"]["rows"], 0)


if __name__ == "__main__":
    unittest.main()
