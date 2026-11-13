#!/usr/bin/env python3
"""Build a deterministic Stage 5 static residual ranking from compact inventories."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
from collections import Counter
from pathlib import Path
from typing import Any

from export_stage5_isa_evidence import normalized_instruction_form


ROOT = Path(__file__).resolve().parents[2]
ROOTFS_DEFAULT = ROOT / "docs/reports/data/stage5/s5-21-isa-inventory.json.gz"
E_DEFAULT = ROOT / "docs/reports/data/stage5/s5-21-isa-inventory-e-static.json.gz"
JSON_DEFAULT = ROOT / "docs/reports/data/stage5/s5-21-ranked-residuals.json"
MARKDOWN_DEFAULT = ROOT / "docs/reports/data/stage5/s5-21-ranked-residuals.md"

ANNOTATION_FIELDS = (
    "conditional_candidate_frequencies",
    "conditional_local_candidate_frequencies",
    "evidence_scope_frequencies",
    "profile_match_frequencies",
)


def _canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _empty_annotations(record: dict[str, Any]) -> bool:
    return all(not record.get(name, {}) for name in ANNOTATION_FIELDS)


def _read_compact(path: Path) -> tuple[dict[str, Any], str]:
    raw = path.read_bytes()
    return json.loads(gzip.decompress(raw)), hashlib.sha256(raw).hexdigest()


def _unclassified_exact(exact: dict[str, Any]) -> bool:
    return exact.get("classification_frequencies") == {"unclassified": exact["count"]}


def _shape_label(dimensions: dict[str, Any]) -> str:
    prefixes = " ".join(dimensions["prefix_bytes"]) or "(none)"
    opcode = " ".join(dimensions["opcode_bytes"])
    modrm = dimensions.get("modrm")
    modrm_label = "none" if modrm is None else f"mod={modrm['mod']},reg={modrm['reg']},rm={modrm['rm']}"
    address = dimensions.get("address", {})
    address_label = f"{address.get('form')}/a{address.get('size')}"
    if address.get("segment"):
        address_label += f"/{address['segment']}"
    return f"{prefixes} | {opcode} | {modrm_label} | {address_label}"


def _scenario_rows(record: dict[str, Any], binaries: set[str]) -> tuple[int, dict[str, int]]:
    selected = {name: count for name, count in record.get("binary_frequencies", {}).items() if name in binaries and count}
    return sum(selected.values()), dict(sorted(selected.items()))


def _ranked_records(
    records: list[dict[str, Any]], binaries: set[str], *, exact: bool, limit: int, scenario: str
) -> list[dict[str, Any]]:
    ranked = []
    for record in records:
        rows, per_binary = _scenario_rows(record, binaries)
        if rows == 0:
            continue
        if exact:
            source = record["dimensions"]
            item = {
                "shape": _shape_label(source),
                "encoding_key": source["encoding_key"],
                "rows": rows,
                "binary_frequencies": per_binary,
            }
        else:
            source = record["instruction_form"]
            item = {
                "operand_signature": source["operand_signature"],
                "semantic_prefixes": source["semantic_prefixes"],
                "opcode_bytes": source["opcode_bytes"],
                "group_extension": source["group_extension"],
                "modrm_mode": source["modrm_mode"],
                "address": source["address"],
                "rows": rows,
                "exact_keys": record["scenario_exact_keys"][scenario],
                "binary_frequencies": per_binary,
            }
        ranked.append(item)
    return sorted(ranked, key=lambda item: (-item["rows"], _canonical(item)))[:limit]


def rank_inventory(compact: dict[str, Any], compact_sha256: str, *, source_label: str) -> dict[str, Any]:
    normalized_by_key = {
        _canonical(record["instruction_form"]): record
        for record in compact["normalized_instruction_forms"]
    }
    exact_by_normalized: dict[str, list[dict[str, Any]]] = {key: [] for key in normalized_by_key}
    for exact in compact["exact_encoding_forms"]:
        normalized = normalized_instruction_form(exact["dimensions"])
        key = _canonical(normalized)
        if key not in exact_by_normalized:
            raise ValueError("exact encoding has no normalized aggregate")
        exact_by_normalized[key].append(exact)

    residual_normalized: list[dict[str, Any]] = []
    residual_exact: list[dict[str, Any]] = []
    for key, aggregate in normalized_by_key.items():
        exact_records = exact_by_normalized[key]
        if not exact_records:
            raise ValueError("normalized aggregate has no exact encodings")
        if aggregate.get("classification_frequencies") != {"unclassified": aggregate["count"]}:
            continue
        if not _empty_annotations(aggregate):
            continue
        if any(not _unclassified_exact(exact) or not _empty_annotations(exact) for exact in exact_records):
            continue
        if sum(exact["count"] for exact in exact_records) != aggregate["count"]:
            raise ValueError("exact and normalized residual counts disagree")
        aggregate_copy = dict(aggregate)
        aggregate_copy["exact_keys"] = len(exact_records)
        residual_normalized.append(aggregate_copy)
        residual_exact.extend(exact_records)

    by_normalized_key = {
        _canonical(record["instruction_form"]): record for record in residual_normalized
    }
    scenario_binaries = {
        name: set(compact["scenarios"][name]["binaries"])
        for name in ("C", "D")
    }
    exact_counts = {name: Counter() for name in scenario_binaries}
    for record in residual_exact:
        key = _canonical(normalized_instruction_form(record["dimensions"]))
        for name, binaries in scenario_binaries.items():
            if _scenario_rows(record, binaries)[0]:
                exact_counts[name][key] += 1
    for key, aggregate in by_normalized_key.items():
        aggregate["scenario_exact_keys"] = {
            name: exact_counts[name][key] for name in scenario_binaries
        }

    scenarios = {}
    for name in ("C", "D"):
        binaries = scenario_binaries[name]
        scenario_rows = sum(_scenario_rows(record, binaries)[0] for record in residual_normalized)
        scenarios[name] = {
            "binaries": sorted(binaries),
            "rows": scenario_rows,
            "normalized_forms": sum(_scenario_rows(record, binaries)[0] > 0 for record in residual_normalized),
            "exact_keys": sum(_scenario_rows(record, binaries)[0] > 0 for record in residual_exact),
            "top_normalized_forms": _ranked_records(residual_normalized, binaries, exact=False, limit=10, scenario=name),
            "top_exact_shapes": _ranked_records(residual_exact, binaries, exact=True, limit=10, scenario=name),
        }

    return {
        "source": source_label,
        "compact_sha256": compact_sha256,
        "residual_filter": {
            "classification": "unclassified-only",
            "requires_empty_maps": list(ANNOTATION_FIELDS),
            "normalized_form_excluded_if_any_exact_key_has_profile_or_evidence": True,
        },
        "totals": {
            "rows": sum(record["count"] for record in residual_normalized),
            "normalized_forms": len(residual_normalized),
            "exact_keys": len(residual_exact),
        },
        "scenarios": scenarios,
    }


def build_report(rootfs_path: Path = ROOTFS_DEFAULT, e_path: Path = E_DEFAULT) -> dict[str, Any]:
    rootfs, rootfs_hash = _read_compact(rootfs_path)
    e_inventory, e_hash = _read_compact(e_path)
    root = rank_inventory(rootfs, rootfs_hash, source_label=rootfs_path.name)
    e = rank_inventory(e_inventory, e_hash, source_label=e_path.name)
    e_delta = {
        key: e["totals"][key] - root["totals"][key]
        for key in ("rows", "normalized_forms", "exact_keys")
    }
    e_delta["scenarios"] = {
        name: {
            key: e["scenarios"][name][key] - root["scenarios"][name][key]
            for key in ("rows", "normalized_forms", "exact_keys")
        }
        for name in ("C", "D")
    }
    return {
        "schema_version": 1,
        "ranking_basis": "static instruction-row coverage opportunity; not runtime frequency, speedup, or implementation-cost estimate",
        "rootfs": root,
        "e_inclusive": e,
        "e_delta": e_delta,
    }


def render_markdown(report: dict[str, Any]) -> str:
    lines = [
        "# S5-21 ranked static residual handoff",
        "",
        "This ranking is derived from the compact static inventories. It measures observed instruction-row coverage opportunity only; it does not estimate runtime frequency, speedup, implementation cost, or support status.",
        "",
        f"Rootfs compact SHA-256: `{report['rootfs']['compact_sha256']}`  ",
        f"E-inclusive compact SHA-256: `{report['e_inclusive']['compact_sha256']}`",
        "",
        "| Population | Rows | Normalized forms | Exact keys |",
        "|---|---:|---:|---:|",
    ]
    for label, value in (("Rootfs strict residual", report["rootfs"]), ("E-inclusive strict residual", report["e_inclusive"])):
        totals = value["totals"]
        lines.append(f"| {label} | {totals['rows']} | {totals['normalized_forms']} | {totals['exact_keys']} |")
    lines.extend([
        f"| E delta | {report['e_delta']['rows']} | {report['e_delta']['normalized_forms']} | {report['e_delta']['exact_keys']} |",
        "",
        "A normalized form is included only when its aggregate is entirely unclassified and has empty conditional-candidate, local-candidate, evidence-scope, and profile-match maps; every exact encoding under it must also satisfy those conditions. A partial exact-profile match excludes the whole normalized form from this residual ranking.",
        "",
        "All rows below remain unclassified. The ranking is a Stage 2/4 handoff, not a claim that the instruction is unsupported.",
    ])
    for name in ("C", "D"):
        scenario = report["rootfs"]["scenarios"][name]
        lines.extend([
            "",
            f"## Scenario {name}",
            "",
            f"Binaries: {', '.join(scenario['binaries'])}. Rootfs residual: {scenario['rows']} rows, {scenario['normalized_forms']} forms, {scenario['exact_keys']} exact keys. E delta: {report['e_delta']['scenarios'][name]['rows']} rows.",
            "",
            "### Top normalized forms",
            "",
            "| # | Signature | Prefixes | Opcode | Group | ModR/M | Address | Rows | Exact keys | Binary distribution |",
            "|---:|---|---|---|---:|---|---|---:|---:|---|",
        ])
        for i, item in enumerate(scenario["top_normalized_forms"], 1):
            prefixes = " ".join(item["semantic_prefixes"]) or "(none)"
            opcode = " ".join(item["opcode_bytes"])
            group = "" if item["group_extension"] is None else str(item["group_extension"])
            address = item["address"]
            address_text = f"{address['form']}/a{address['size']}"
            if address.get("segment"):
                address_text += f"/{address['segment']}"
            dist = ", ".join(f"{binary}={count}" for binary, count in item["binary_frequencies"].items())
            lines.append(f"| {i} | `{item['operand_signature']}` | `{prefixes}` | `{opcode}` | {group} | {item['modrm_mode']} | `{address_text}` | {item['rows']} | {item['exact_keys']} | {dist} |")
        lines.extend([
            "",
            "### Top exact encoding shapes",
            "",
            "| # | Prefix bytes | Opcode | ModR/M | Address shape | Rows | Binary distribution |",
            "|---:|---|---|---|---|---:|---|",
        ])
        for i, item in enumerate(scenario["top_exact_shapes"], 1):
            pieces = item["shape"].split(" | ")
            dist = ", ".join(f"{binary}={count}" for binary, count in item["binary_frequencies"].items())
            lines.append(f"| {i} | `{pieces[0]}` | `{pieces[1]}` | `{pieces[2]}` | `{pieces[3]}` | {item['rows']} | {dist} |")
    lines.append("")
    return "\n".join(lines)


def write_outputs(rootfs_path: Path, e_path: Path, json_path: Path, markdown_path: Path) -> None:
    report = build_report(rootfs_path, e_path)
    json_path.parent.mkdir(parents=True, exist_ok=True)
    markdown_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(json.dumps(report, sort_keys=True, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    markdown_path.write_text(render_markdown(report), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rootfs", type=Path, default=ROOTFS_DEFAULT)
    parser.add_argument("--e-inclusive", type=Path, default=E_DEFAULT)
    parser.add_argument("--json-out", type=Path, default=JSON_DEFAULT)
    parser.add_argument("--markdown-out", type=Path, default=MARKDOWN_DEFAULT)
    args = parser.parse_args()
    write_outputs(args.rootfs, args.e_inclusive, args.json_out, args.markdown_out)
    print(f"json={args.json_out}\nmarkdown={args.markdown_out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
