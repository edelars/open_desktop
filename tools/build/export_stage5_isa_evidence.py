#!/usr/bin/env python3
"""Create compact, deterministic Stage 5 ISA evidence from a full inventory."""

from __future__ import annotations

import argparse
import gzip
import json
from collections import Counter
from pathlib import Path


def semantic_prefixes(prefix_bytes: list[str]) -> list[str]:
    """Keep semantic prefix order while normalizing REX register extensions."""
    result = []
    for raw in prefix_bytes:
        value = int(raw, 16)
        if value == 0xF0:
            name = "lock"
        elif value == 0xF2:
            name = "repne"
        elif value == 0xF3:
            name = "rep"
        elif value == 0x66:
            name = "operand_size"
        elif value == 0x67:
            name = "address_size"
        elif value in {0x26, 0x2E, 0x36, 0x3E, 0x64, 0x65}:
            name = {0x26: "es", 0x2E: "cs", 0x36: "ss", 0x3E: "ds", 0x64: "fs", 0x65: "gs"}[value]
        elif 0x40 <= value <= 0x4F:
            name = "rex_w" if value & 8 else "rex"
        else:
            name = f"prefix_{raw.lower()}"
        result.append(name)
    return result


def normalized_instruction_form(dimensions: dict) -> dict:
    modrm = dimensions["modrm"]
    opcode_bytes = dimensions["opcode_bytes"]
    one_byte_groups = {"80", "81", "82", "83", "F6", "F7", "FE", "FF", "C0", "C1", "D0", "D1", "D2", "D3"}
    two_byte_groups = {("0F", "01")}
    is_group = modrm and ((len(opcode_bytes) == 1 and opcode_bytes[0] in one_byte_groups) or tuple(opcode_bytes) in two_byte_groups)
    group = modrm["reg"] if is_group else None
    normalized_opcode = list(opcode_bytes)
    if len(normalized_opcode) == 1:
        value = int(normalized_opcode[0], 16)
        if 0x50 <= value <= 0x57: normalized_opcode = ["50"]
        elif 0x58 <= value <= 0x5F: normalized_opcode = ["58"]
        elif 0x90 <= value <= 0x97: normalized_opcode = ["90"]
        elif 0xB0 <= value <= 0xB7: normalized_opcode = ["B0"]
        elif 0xB8 <= value <= 0xBF: normalized_opcode = ["B8"]
    elif len(normalized_opcode) == 2 and normalized_opcode[0] == "0F" and 0xC8 <= int(normalized_opcode[1], 16) <= 0xCF:
        normalized_opcode = ["0F", "C8"]
    return {
        "semantic_prefixes": semantic_prefixes(dimensions["prefix_bytes"]),
        "opcode_bytes": normalized_opcode,
        "group_extension": group,
        "operand_signature": dimensions["operand_signature"],
        "modrm_mode": None if modrm is None else ("register" if modrm["mod"] == 3 else "memory"),
        "address": dimensions["address"],
    }


def aggregate(inventory: dict) -> dict:
    exact_forms = inventory["inventory"]["encoding_forms"]
    groups: dict[str, dict] = {}
    for exact in exact_forms:
        normalized = normalized_instruction_form(exact["dimensions"])
        key = json.dumps(normalized, sort_keys=True, separators=(",", ":"))
        target = groups.setdefault(key, {
            "instruction_form": normalized,
            "count": 0,
            "binary_frequencies": Counter(),
            "binary_classification_frequencies": {},
            "classification_frequencies": Counter(),
            "evidence_scope_frequencies": Counter(),
            "profile_match_frequencies": Counter(),
            "conditional_candidate_frequencies": Counter(),
        })
        target["count"] += exact["count"]
        target["binary_frequencies"].update(exact["binary_frequencies"])
        exact_classes = exact["classification_frequencies"]
        if len(exact_classes) != 1 or sum(exact_classes.values()) != exact["count"]:
            raise ValueError("an exact encoding key has non-deterministic classification")
        exact_class = next(iter(exact_classes))
        for name, count in exact["binary_frequencies"].items():
            by_class = target["binary_classification_frequencies"].setdefault(name, Counter())
            by_class[exact_class] += count
        target["classification_frequencies"].update(exact["classification_frequencies"])
        target["evidence_scope_frequencies"].update(exact["evidence_scope_frequencies"])
        target["profile_match_frequencies"].update(exact["profile_match_frequencies"])
        target["conditional_candidate_frequencies"].update(exact["conditional_candidate_frequencies"])
    normalized_forms = []
    for key, item in sorted(groups.items(), key=lambda pair: (-pair[1]["count"], pair[0])):
        normalized_forms.append({
            "instruction_form": item["instruction_form"],
            "count": item["count"],
            "binary_frequencies": dict(sorted(item["binary_frequencies"].items())),
            "binary_classification_frequencies": {
                name: dict(sorted(counts.items()))
                for name, counts in sorted(item["binary_classification_frequencies"].items())
            },
            "classification_frequencies": dict(sorted(item["classification_frequencies"].items())),
            "evidence_scope_frequencies": dict(sorted(item["evidence_scope_frequencies"].items())),
            "profile_match_frequencies": dict(sorted(item["profile_match_frequencies"].items())),
            "conditional_candidate_frequencies": dict(sorted(item["conditional_candidate_frequencies"].items())),
        })
    by_binary = {}
    for binary in inventory["binaries"]:
        records = [row for row in normalized_forms if binary["name"] in row["binary_frequencies"]]
        unclassified = sorted(
            ((row["instruction_form"], row["binary_classification_frequencies"][binary["name"]].get("unclassified", 0)) for row in records
             if row["binary_classification_frequencies"][binary["name"]].get("unclassified", 0) > 0),
            key=lambda pair: (-pair[1], json.dumps(pair[0], sort_keys=True)),
        )
        by_binary[binary["name"]] = {
            "image_path": binary["image_path"],
            "size_bytes": binary["size_bytes"],
            "sha256": binary["sha256"],
            "elf_machine": binary["elf_machine"],
            "build_id": binary["build_id"],
            "instruction_count": binary["instruction_count"],
            "classification_frequencies": binary["classification_frequencies"],
            "unique_exact_encoding_forms": sum(binary["name"] in row["binary_frequencies"] for row in exact_forms),
            "unique_normalized_instruction_forms": len(records),
            "top_unclassified_forms": [
                {"instruction_form": form, "count": count} for form, count in unclassified[:25]
            ],
        }
    scenarios = {}
    scenario_binaries = {
        "C": ["bash", "dash", "tar"],
        "D": ["bash", "dash", "gcc-driver", "gcc-cc1", "gcc-collect2", "gcc-as", "gcc-ld-bfd"],
    }
    for scenario, names in scenario_binaries.items():
        rows = []
        for form in normalized_forms:
            count = sum(form["binary_frequencies"].get(name, 0) for name in names)
            count = sum(form["binary_classification_frequencies"].get(name, {}).get("unclassified", 0) for name in names)
            if count:
                rows.append((form["instruction_form"], count))
        rows.sort(key=lambda pair: (-pair[1], json.dumps(pair[0], sort_keys=True)))
        scenarios[scenario] = {
            "binaries": names,
            "unclassified_instruction_rows": sum(count for _, count in rows),
            "conditional_candidate_instruction_rows": sum(
                form["binary_frequencies"].get(name, 0)
                for form in exact_forms
                if form["conditional_candidate_frequencies"]
                for name in names
            ),
            "conditional_candidate_frequencies": {
                candidate: sum(
                    form["binary_frequencies"].get(name, 0)
                    for form in exact_forms
                    if candidate in form["conditional_candidate_frequencies"]
                    for name in names
                )
                for candidate in sorted({
                    key for form in exact_forms for key in form["conditional_candidate_frequencies"]
                })
            },
            "top_unclassified_forms": [{"instruction_form": form, "count": count} for form, count in rows[:25]],
        }
    e_static = by_binary.get("stage5-e-static")
    e_forms = []
    if e_static is not None:
        for form in normalized_forms:
            count = form["binary_classification_frequencies"].get("stage5-e-static", {}).get("unclassified", 0)
            if count:
                e_forms.append((form["instruction_form"], count))
        e_forms.sort(key=lambda pair: (-pair[1], json.dumps(pair[0], sort_keys=True)))
    scenarios["E"] = {
        "status": "static_executable_only" if e_static else "static_executable_not_built",
        "static_executable": None if e_static is None else {
            "binary": "stage5-e-static",
            "image_path": e_static["image_path"],
            "sha256": e_static["sha256"],
            "size_bytes": e_static["size_bytes"],
            "elf_machine": e_static["elf_machine"],
            "build_id": e_static["build_id"],
            "instruction_count": e_static["instruction_count"],
            "classification_frequencies": e_static["classification_frequencies"],
            "unclassified_instruction_rows": sum(count for _, count in e_forms),
            "top_unclassified_forms": [{"instruction_form": form, "count": count} for form, count in e_forms[:25]],
        },
        "runtime_generated_code": {
            "status": "not_inferred",
            "frequency": "not_inferred",
            "instruction_rows": None,
            "reason": "Static executable disassembly does not execute the E program or observe its runtime-generated code bytes.",
        },
    }
    return {
        "schema_version": 1,
        "source_rootfs_sha256": inventory["source_image"]["sha256"],
        "source_image": inventory["source_image"],
        "external_inputs": inventory.get("external_inputs", []),
        "disassembler": inventory["disassembler"],
        "profile_comparison": inventory["profile_comparison"],
        "instruction_count": inventory["inventory"]["instruction_count"],
        "exact_encoding_form_count": len(exact_forms),
        "normalized_instruction_form_count": len(normalized_forms),
        "classification_frequencies": inventory["inventory"]["classification_frequencies"],
        "evidence_scope_frequencies": inventory["inventory"]["evidence_scope_frequencies"],
        "exact_instruction_rows_with_any_conditional_candidate": sum(
            item["count"] for item in exact_forms if item["conditional_candidate_frequencies"]
        ),
        "conditional_cpl0_candidate_frequencies": inventory["inventory"]["conditional_cpl0_candidate_frequencies"],
        "conditional_candidate_hit_count": sum(inventory["inventory"]["conditional_cpl0_candidate_frequencies"].values()),
        "conditional_candidate_supported_overlap_rows": inventory.get("conditional_candidates", {}).get("supported_overlap_rows"),
        "exact_encoding_forms": [
            {
                "dimensions": item["dimensions"],
                "count": item["count"],
                "binary_frequencies": item["binary_frequencies"],
                "classification_frequencies": item["classification_frequencies"],
                "evidence_scope_frequencies": item["evidence_scope_frequencies"],
                "profile_match_frequencies": item["profile_match_frequencies"],
                "conditional_candidate_frequencies": item["conditional_candidate_frequencies"],
            }
            for item in exact_forms
        ],
        "normalized_instruction_forms": normalized_forms,
        "binaries": by_binary,
        "scenarios": scenarios,
    }


def write_gzip(source: Path, destination: Path) -> None:
    report = aggregate(json.loads(source.read_text()))
    payload = (json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n").encode()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("xb") as raw:
        with gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0, compresslevel=9) as stream:
            stream.write(payload)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inventory", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    write_gzip(args.inventory, args.output)
    print(f"output={args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
