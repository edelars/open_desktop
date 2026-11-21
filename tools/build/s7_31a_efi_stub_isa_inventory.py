#!/usr/bin/env python3
"""S7-31a: static inventory of the Ubuntu EFI stub against the firmware profile and the CPL0 contract.

Not evidence of execution. Reuses the S4-38 L2 disassembly, recursive-descent and CPU_CONTRACT status
helpers (tools/build/s4_38l2_isa_inventory.py), which scan the decompressor from .Lrelocated, and adds
a second walk from the PE entry point (efi_pe_entry). The EFI stub is the code reachable from that
entry and not from .Lrelocated. Two verdicts are produced per distinct instruction encoding:

* firmware profile: the od-isa example firmware_profile_probe runs `step_paged_firmware_probe` on the bytes;
  "unsupported" is `PagedProbeExit::Unsupported`, anything else is "decoded" (decode level only);
* CPL0 contract: `status_for` of s4_38l2_contract_status.py (hand-assessed in S4-38 L2, may be stale).

Reachability is a recursive descent over a linear disassembly: indirect transfers are counted and not
followed (lower bound), branches that are not taken at run time are followed (upper bound).
Exit codes: 0 written, 2 usage or input error.
"""

from __future__ import annotations

import argparse
import collections
import json
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s4_38l2_isa_inventory as inv  # noqa: E402
import s4_38l2_oracle as oracle  # noqa: E402
from s4_38l2_contract_status import status_for  # noqa: E402

ROOT = oracle.ROOT
DEFAULT_OUT = ROOT / "docs/reports/data/stage7/s7-31a-efi-stub-isa-inventory.json"
DEFAULT_PROBE = ROOT / "target/release/examples/firmware_profile_probe"
TOP = 15


def pe_entry_rva(image: bytes) -> int:
    pe = struct.unpack_from("<I", image, 0x3C)[0]
    return struct.unpack_from("<I", image, pe + 24 + 16)[0]


def probe(binary: Path, encodings: list[bytes]) -> dict[bytes, tuple[str, str]]:
    text = "".join(f"{code.hex()}\n" for code in encodings)
    done = subprocess.run([binary], input=text, capture_output=True, text=True, check=True)
    verdicts = {}
    for line in done.stdout.splitlines():
        digits, verdict, kind = line.split()
        verdicts[bytes.fromhex(digits)] = (verdict, kind)
    if len(verdicts) != len(set(encodings)):
        raise RuntimeError("the probe did not answer every encoding")
    return verdicts


def build(image: bytes, objdump: str, work: Path, binary: Path) -> dict:
    header = oracle.parse_setup_header(image)
    pm = image[header["pm_offset"]:]
    reloc = oracle.relocated_copy_address(pm, header, header["pref_address"])
    text_start = reloc["rva_Lrelocated_imm32"]
    entry = pe_entry_rva(image) - header["pm_offset"]
    if not text_start <= entry < len(pm):
        raise ValueError(f"PE entry (pm offset {entry:#x}) is outside the decompressor text region")
    text = pm[text_start:]
    elf = work / "stub-text.elf"
    elf.write_bytes(inv.wrap_as_elf(text, inv.LOAD_ADDRESS + text_start))
    insns = inv.disassemble(elf, objdump, inv.LOAD_ADDRESS + text_start, inv.LOAD_ADDRESS + len(pm))
    by_addr = {i["addr"]: i for i in insns}
    noreturn = inv.find_noreturn(insns)
    seen_stub, info_stub = inv.walk(by_addr, [inv.LOAD_ADDRESS + entry], noreturn=noreturn)
    seen_dec, info_dec = inv.walk(by_addr, [inv.LOAD_ADDRESS + text_start], noreturn=noreturn)
    stub_only = sorted(seen_stub - seen_dec)
    verdicts = probe(binary, sorted({by_addr[a]["bytes"] for a in seen_stub | seen_dec}))

    def firmware_summary(addresses: list[int]) -> dict:
        unsupported = [by_addr[a] for a in addresses if verdicts[by_addr[a]["bytes"]][0] == "unsupported"]
        encodings = {i["bytes"] for i in unsupported}
        distinct = {by_addr[a]["bytes"] for a in addresses}
        by_mnemonic = collections.Counter(inv.mnemonic_of(i["asm"]) for i in unsupported)
        return {
            "instructions": len(addresses),
            "unsupported_instructions": len(unsupported),
            "unsupported_share": round(len(unsupported) / len(addresses), 4) if addresses else None,
            "distinct_encodings": len(distinct),
            "unsupported_distinct_encodings": len(encodings),
            "exit_kinds_of_decoded": dict(collections.Counter(
                verdicts[by_addr[a]["bytes"]][1] for a in addresses
                if verdicts[by_addr[a]["bytes"]][0] == "decoded")),
            "top_unsupported_mnemonics": by_mnemonic.most_common(TOP),
        }

    forms: dict[str, dict] = {}
    inv.add_forms(forms, [by_addr[a] for a in stub_only], "efi_stub_only", lambda a: a)
    rows = list(forms.values())
    status_instructions: collections.Counter = collections.Counter()
    status_forms: collections.Counter = collections.Counter()
    uncovered = []
    for row in rows:
        row.update(status_for(row))
        status_forms[row["status"]] += 1
        status_instructions[row["status"]] += row["count"]
        if row["status"] in inv.NOT_COVERED:
            uncovered.append({"family": row["family"], "status": row["status"], "count": row["count"],
                              "example_bytes": row["example_bytes"], "example_asm": row["example_asm"]})
    uncovered.sort(key=lambda r: -r["count"])
    return {
        "schema": "od-s7-31a-efi-stub-isa-inventory-v1",
        "note": "static estimate, not evidence of execution; see the module docstring",
        "pe_entry_pm_offset": hex(entry),
        "decompressor_text_start_pm_offset": hex(text_start),
        "linear_instructions_in_text": len(insns),
        "indirect_sites_from_entry": len(info_stub["indirect_sites"]),
        "outside_targets_from_entry": len(info_stub["outside_targets"]),
        "stub_reachable_from_entry": firmware_summary(sorted(seen_stub)),
        "stub_only_not_reachable_from_Lrelocated": firmware_summary(stub_only),
        "decompressor_from_Lrelocated": firmware_summary(sorted(seen_dec)),
        "cpl0_contract_stub_only": {
            "forms_by_status": dict(status_forms),
            "instructions_by_status": dict(status_instructions),
            "status_meaning": "exact and family are covered by CPU_CONTRACT rules; absent, partial and "
                              "not_checked are the forms the contract does not admit or does not name",
            "uncovered_forms_by_count": uncovered,
            "uncovered_forms_total": len(uncovered),
            "uncovered_instructions_total": sum(row["count"] for row in uncovered),
        },
        "unsupported_by_firmware_profile_stub_only_examples": [
            {"bytes": by_addr[a]["bytes"].hex(), "asm": by_addr[a]["asm"]}
            for a in stub_only if verdicts[by_addr[a]["bytes"]][0] == "unsupported"
        ][:25],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--vmlinuz", type=Path, required=True)
    parser.add_argument("--probe", type=Path, default=DEFAULT_PROBE,
                        help="cargo build --release -p od-isa --example firmware_profile_probe")
    parser.add_argument("--objdump", default=inv.DEFAULT_OBJDUMP)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args(argv)
    if not args.vmlinuz.is_file() or not args.probe.is_file():
        print("error: --vmlinuz and --probe must exist", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="od-s7-31a-") as directory:
        result = build(args.vmlinuz.read_bytes(), args.objdump, Path(directory), args.probe)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({k: result[k] for k in ("stub_reachable_from_entry", "stub_only_not_reachable_from_Lrelocated")},
                     indent=1)[:3000])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
