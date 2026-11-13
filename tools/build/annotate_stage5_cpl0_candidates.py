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
        "S4-38L6a-PREFETCHNTA-base-only": ("prefetchnta mem8", ["0F", "18"], []),
        "S4-38L6a-PREFETCHT0-base-only": ("prefetcht0 mem8", ["0F", "18"], []),
        "S4-38L4g-REP-BSF-register-BMI1-clear": ("tzcnt register operands; BSF when BMI1=0", ["0F", "BC"], ["F3"]),
        "S4-38L7ay-LOCK-XADD-m32-r32-base-only": ("lock xadd mem32,r32", ["0F", "C1"], ["F0"]),
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
            matches = (form["operand_signature"], form["opcode_bytes"], form["semantic_prefixes"]) == (
                operand, opcode, ["rep"] if prefixes == ["F3"] else []
            )
            if candidate == "S4-38L4g-REP-BSF-register-BMI1-clear":
                signature = form["operand_signature"]
                allowed_prefixes = {
                    "tzcnt r32,r32": (["rep"], ["rep", "rex"]),
                    "tzcnt r64,r64": (["rep", "rex_w"],),
                }
                matches = (
                    signature in allowed_prefixes
                    and form["opcode_bytes"] == ["0F", "BC"]
                    and form["semantic_prefixes"] in allowed_prefixes.get(signature, ())
                    and form["modrm_mode"] == "register"
                    and form["address"] == {"form": "none", "segment": None, "size": None}
                    and form["group_extension"] is None
                )
            if candidate == "S4-38L7ay-LOCK-XADD-m32-r32-base-only":
                matches = (
                    form["operand_signature"] == "lock xadd mem32,r32"
                    and form["opcode_bytes"] == ["0F", "C1"]
                    and form["semantic_prefixes"] == ["lock"]
                    and form["modrm_mode"] == "memory"
                    and form["address"] == {"form": "base_only", "segment": None, "size": 64}
                    and form["group_extension"] is None
                )
            if candidate.startswith("S4-38L6a-PREFETCH"):
                matches = matches and form["address"] == {"form": "base_only", "segment": None, "size": 64} and form["modrm_mode"] == "memory" and form["group_extension"] is None
            if matches:
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
            matches = (dimensions["operand_signature"], dimensions["opcode_bytes"], dimensions["prefix_bytes"]) == (operand, opcode, prefixes)
            if candidate == "S4-38L4g-REP-BSF-register-BMI1-clear":
                observed_prefixes = dimensions["prefix_bytes"]
                signature = dimensions["operand_signature"]
                rex_width_matches = signature == "tzcnt r32,r32" and len(observed_prefixes) == 1
                if len(observed_prefixes) == 2:
                    rex = int(observed_prefixes[1], 16)
                    rex_width_matches = (
                        (signature == "tzcnt r64,r64" and rex & 0xF8 == 0x48)
                        or (signature == "tzcnt r32,r32" and rex & 0xF8 == 0x40)
                    )
                rex_is_canonical = (
                    len(observed_prefixes) in (1, 2)
                    and observed_prefixes[0] == "F3"
                    and (len(observed_prefixes) == 1 or int(observed_prefixes[1], 16) & 0xF0 == 0x40)
                )
                matches = (
                    signature in {"tzcnt r32,r32", "tzcnt r64,r64"}
                    and dimensions["opcode_bytes"] == ["0F", "BC"]
                    and rex_is_canonical
                    and rex_width_matches
                    and dimensions["instruction_length"] == 4 + len(observed_prefixes) - 1
                    and dimensions["modrm"]["mod"] == 3
                    and dimensions["address"] == {"form": "none", "segment": None, "size": None}
                    and dimensions["sib"] is None
                )
            if candidate == "S4-38L7ay-LOCK-XADD-m32-r32-base-only":
                modrm = dimensions["modrm"]
                observed_modrm_registers = {
                    "48": (1, 0), "50": (2, 0), "47": (0, 7),
                    "4A": (1, 2), "51": (2, 1), "56": (2, 6),
                }
                matches = (
                    dimensions["operand_signature"] == "lock xadd mem32,r32"
                    and dimensions["opcode_bytes"] == ["0F", "C1"]
                    and dimensions["prefix_bytes"] == ["F0"]
                    and dimensions["instruction_length"] == 5
                    and modrm["mod"] == 1
                    and modrm["raw"] in observed_modrm_registers
                    and (modrm["reg"], modrm["rm"]) == observed_modrm_registers.get(modrm["raw"])
                    and dimensions["address"] == {"form": "base_only", "segment": None, "size": 64}
                    and dimensions["sib"] is None
                )
            if candidate.startswith("S4-38L6a-PREFETCH"):
                reg = 0 if candidate.endswith("PREFETCHNTA-base-only") else 1
                matches = matches and dimensions == {
                    "address": {"form": "base_only", "segment": None, "size": 64},
                    "encoding_key": dimensions["encoding_key"],
                    "instruction_length": 3,
                    "modrm": {"mod": 0, "raw": "00" if reg == 0 else "08", "reg": reg, "rm": 0},
                    "opcode_bytes": ["0F", "18"],
                    "operand_signature": operand,
                    "prefix_bytes": [],
                    "sib": None,
                }
            if matches:
                record["conditional_candidate_frequencies"][candidate] = record["count"]
                forms[candidate] = forms.get(candidate, 0) + record["count"]
                for binary, count in record["binary_frequencies"].items():
                    candidate_rows_by_binary.setdefault(candidate, Counter())[binary] += count
                if record["classification_frequencies"] != {"unclassified": record["count"]} or record["evidence_scope_frequencies"]:
                    raise ValueError(f"candidate {candidate} must remain unclassified/outside evidence scope")
    expected = {
        "S4-38L9b-PAUSE-exact-F3-90": 6,
        "S4-38L6d-LEAVE-bare-C9": 2005,
        "S4-38L6a-PREFETCHNTA-base-only": 3,
        "S4-38L6a-PREFETCHT0-base-only": 3,
        "S4-38L4g-REP-BSF-register-BMI1-clear": 398,
        "S4-38L7ay-LOCK-XADD-m32-r32-base-only": 36,
    }
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
    for scenario in data.get("scenarios", {}).values():
        if "binaries" not in scenario:
            continue
        binaries = scenario["binaries"]
        cpl0 = Counter()
        local = Counter()
        candidate_instruction_rows = 0
        for item in data["exact_encoding_forms"]:
            binary_rows = sum(item["binary_frequencies"].get(binary, 0) for binary in binaries)
            cpl0_candidates = item["conditional_candidate_frequencies"]
            local_candidates = item.get("conditional_local_candidate_frequencies", {})
            if cpl0_candidates or local_candidates:
                candidate_instruction_rows += binary_rows
            for candidate in cpl0_candidates:
                cpl0[candidate] += binary_rows
            for candidate in local_candidates:
                local[candidate] += binary_rows
        all_cpl0_candidates = {
            candidate for item in data["exact_encoding_forms"]
            for candidate in item["conditional_candidate_frequencies"]
        }
        all_local_candidates = {
            candidate for item in data["exact_encoding_forms"]
            for candidate in item.get("conditional_local_candidate_frequencies", {})
        }
        scenario["conditional_candidate_frequencies"] = {
            candidate: cpl0[candidate] for candidate in sorted(all_cpl0_candidates)
        }
        scenario["conditional_local_candidate_frequencies"] = {
            candidate: local[candidate] for candidate in sorted(all_local_candidates)
        }
        scenario["conditional_candidate_instruction_rows"] = candidate_instruction_rows
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
