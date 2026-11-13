#!/usr/bin/env python3
"""Annotate exact observed Stage 4 CPL0 candidates in compact S5-21 evidence.

This is a compact-only derivation for legacy raw analyzer snapshots. It never
changes instruction classifications, profile matches, or evidence scopes.
"""
from __future__ import annotations

import argparse
from collections import Counter
import gzip
import json
from pathlib import Path


def annotate(path: Path) -> None:
    data = json.loads(gzip.decompress(path.read_bytes()))
    candidates = {
        "S4-38L9b-PAUSE-exact-F3-90": ("pause", ["90"], ["F3"]),
        "S4-38L6d-LEAVE-bare-C9": ("leave", ["C9"], []),
    }
    candidate_ids = set(candidates)
    forms: dict[str, int] = {}
    candidate_rows_by_binary: dict[str, Counter[str]] = {}
    # Make the transform repeatable: discard only this transform's prior labels.
    for record in data["normalized_instruction_forms"]:
        record["conditional_candidate_frequencies"] = {
            key: value for key, value in record["conditional_candidate_frequencies"].items()
            if key not in candidate_ids
        }
        form = record["instruction_form"]
        for candidate, (operand, opcode, prefixes) in candidates.items():
            if (form["operand_signature"], form["opcode_bytes"], form["semantic_prefixes"]) == (
                operand, opcode, ["rep"] if prefixes == ["F3"] else []
            ):
                record["conditional_candidate_frequencies"][candidate] = record["count"]
                if record["classification_frequencies"] != {"unclassified": record["count"]} or record["evidence_scope_frequencies"]:
                    raise ValueError(f"candidate {candidate} must remain unclassified/outside evidence scope")
    for record in data["exact_encoding_forms"]:
        record["conditional_candidate_frequencies"] = {
            key: value for key, value in record["conditional_candidate_frequencies"].items()
            if key not in candidate_ids
        }
        dimensions = record["dimensions"]
        for candidate, (operand, opcode, prefixes) in candidates.items():
            if (dimensions["operand_signature"], dimensions["opcode_bytes"], dimensions["prefix_bytes"]) == (operand, opcode, prefixes):
                record["conditional_candidate_frequencies"][candidate] = record["count"]
                forms[candidate] = forms.get(candidate, 0) + record["count"]
                for binary, count in record["binary_frequencies"].items():
                    candidate_rows_by_binary.setdefault(candidate, Counter())[binary] += count
                if record["classification_frequencies"] != {"unclassified": record["count"]} or record["evidence_scope_frequencies"]:
                    raise ValueError(f"candidate {candidate} must remain unclassified/outside evidence scope")
    expected = {"S4-38L9b-PAUSE-exact-F3-90": 6, "S4-38L6d-LEAVE-bare-C9": 2005}
    if forms != expected:
        raise ValueError(f"observed candidate counts changed: {forms}")
    for binary, record in data["binaries"].items():
        frequencies = record.setdefault("conditional_cpl0_candidate_frequencies", {})
        for candidate in candidate_ids:
            frequencies.pop(candidate, None)
        for candidate, counts in candidate_rows_by_binary.items():
            if binary in counts:
                frequencies[candidate] = counts[binary]

    # Refresh compact candidate summaries from the exact-form records.
    cpl0_frequencies = Counter()
    for record in data["exact_encoding_forms"]:
        cpl0_frequencies.update(record["conditional_candidate_frequencies"])
    data["conditional_cpl0_candidate_frequencies"] = dict(sorted(cpl0_frequencies.items()))
    data["conditional_cpl0_candidate_hit_count"] = sum(data["conditional_cpl0_candidate_frequencies"].values())
    data["conditional_candidate_hit_count"] = data["conditional_cpl0_candidate_hit_count"] + sum(data["conditional_local_state_gated_candidate_frequencies"].values())
    data["exact_instruction_rows_with_any_conditional_candidate"] = sum(
        item["count"] for item in data["exact_encoding_forms"]
        if item["conditional_candidate_frequencies"] or item.get("conditional_local_candidate_frequencies")
    )
    payload = json.dumps(data, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    path.write_bytes(gzip.compress(payload, mtime=0))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("inventory", nargs="+", type=Path)
    args = parser.parse_args()
    for path in args.inventory:
        annotate(path)


if __name__ == "__main__":
    main()
