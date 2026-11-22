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
Exit codes: 0 written, 2 usage or input error, 3 (ROM mode) calibration failed, 4 (ROM mode) output too large.

ROM mode (S7-31b, `--rom-elf`): the same two verdicts over every instruction of the executable sections of the
firmware ELF (ADR-0013, "Checks"). Only instructions inside sized function symbols count; padding and data in .text are reported apart. The probe runs in
the board context (CR3, guest FP, RDTSC); forms only the bare probe rejects are listed as context artifacts. The
calibration instruction (default: the first at e_entry; or --calibration-bytes/--calibration-symbol/--calibration-expect)
must be found in the named function and get the expected verdict, otherwise nothing is written.
"""

from __future__ import annotations

import argparse
import bisect
import collections
import json
import re
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


def probe(binary: Path, encodings: list[bytes], board_context: bool = False) -> dict[bytes, tuple[str, str]]:
    text = "".join(f"{code.hex()}\n" for code in encodings)
    command = [binary, "--board-context"] if board_context else [binary]
    done = subprocess.run(command, input=text, capture_output=True, text=True, check=True)
    verdicts = {}
    for line in done.stdout.splitlines():
        digits, verdict, kind = line.split()
        verdicts[bytes.fromhex(digits)] = (verdict, kind)
    if len(verdicts) != len(set(encodings)):
        raise RuntimeError("the probe did not answer every encoding")
    return verdicts


def summarize_firmware(addresses: list[int], by_addr: dict[int, dict], verdicts: dict) -> dict:
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
        return summarize_firmware(addresses, by_addr, verdicts)

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


SHF_EXECINSTR = 0x4
STT_FUNC = 2
# Calibration: a known instruction must be found inside a named function and get the expected verdict from
# the probe, otherwise the disassembly, the symbol table or the probe is wrong. The default is the first
# instruction at e_entry (start.S, which runs on every boot and must decode). The S7-31b run also used the
# ISSUE-621 form: --calibration-bytes 4883642408fe --calibration-symbol load_into --calibration-expect unsupported
# (`and qword ptr [rsp+8], -2` in load_into).
DEFAULT_ROM_OUT = ROOT / "docs/reports/data/stage7/s7-31b-rom-isa-inventory.json"
MAX_ROWS = 150
MAX_OUTPUT_BYTES = 200 * 1024


def demangle_v0(name: str) -> str:
    """Identifiers of a Rust v0 path (`_RNvNtNtCs<hash>_<len><id>...`) joined by `::`; generic arguments are cut off."""
    parts, pos = [], 2
    while pos < len(name):
        if name[pos] == "N" and pos + 1 < len(name):
            pos += 2  # namespace tag (`v` value, `t` type, ...)
        elif name[pos] == "I":
            pos += 1  # generic instantiation prefix; the argument list after the path is cut off below
        elif name[pos] == "M":
            pos += 1  # inherent impl: `M` [`s<base62>`] `_`
            if name[pos:pos + 1] == "s":
                pos = name.index("_", pos) + 1
        elif name[pos] == "B":
            pos = name.index("_", pos) + 1  # back reference to an earlier path
        elif name[pos] == "C":
            pos += 1
            if name[pos:pos + 1] == "s":  # crate disambiguator: `s<base62>_`
                pos = name.index("_", pos) + 1
        elif name[pos].isdigit():
            end = pos
            while name[end].isdigit():
                end += 1
            length = int(name[pos:end])
            start = end + (name[end:end + 1] == "_")
            parts.append(name[start:start + length])
            pos = start + length
        else:
            break
    return "::".join(parts)[:160] if parts else name


def demangle(name: str) -> str:
    """Legacy Rust mangling `_ZN<len><id>...E` to `a::b::c` without the hash; other names are returned as is."""
    if name.startswith("_R"):
        return demangle_v0(name)
    if not name.startswith("_ZN"):
        return name
    parts, pos = [], 3
    while pos < len(name) and name[pos].isdigit():
        end = pos
        while name[end].isdigit():
            end += 1
        length = int(name[pos:end])
        parts.append(name[end:end + length])
        pos = end + length
    if pos >= len(name) or name[pos] != "E":
        return name
    if parts and re.fullmatch(r"h[0-9a-f]{16}", parts[-1]):
        parts.pop()
    return "::".join(parts)


def executable_sections(image: bytes) -> list[dict]:
    if len(image) < 64 or image[:4] != b"\x7fELF" or image[4] != 2 or image[5] != 1:
        raise ValueError("not an ELF64 little-endian file")
    shoff = struct.unpack_from("<Q", image, 40)[0]
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", image, 58)
    headers = [struct.unpack_from("<IIQQQQIIQQ", image, shoff + i * shentsize) for i in range(shnum)]
    names = headers[shstrndx]
    result = []
    for name, sh_type, flags, addr, offset, size, *_ in headers:
        if sh_type == 1 and flags & SHF_EXECINSTR and size:
            end = image.index(b"\x00", names[4] + name)
            result.append({"name": image[names[4] + name:end].decode("ascii"), "addr": addr, "size": size})
    return result


def function_ranges(elf: oracle.Elf64) -> list[tuple[int, int, str]]:
    ranges = sorted(
        (symbol["value"], symbol["value"] + symbol["size"], demangle(name))
        for name, symbols in elf.symbols(None).items()
        for symbol in symbols
        if symbol["type"] == STT_FUNC and symbol["size"]
    )
    return ranges


def locate_function(ranges: list[tuple[int, int, str]], starts: list[int], address: int) -> str | None:
    index = bisect.bisect_right(starts, address) - 1
    if index >= 0 and address < ranges[index][1]:
        return ranges[index][2]
    return None


def calibrate(insns: list[dict], function_of, verdicts: dict, code: bytes, symbol: str, expect: str) -> dict:
    """Find `code` inside the ROM functions whose name has the component `symbol`; the probe must answer `expect`."""
    found = [i for i in insns if i["bytes"] == code and symbol in (function_of(i["addr"]) or "").split("::")]
    verdict = verdicts.get(code)
    actual = verdict[0] if verdict else None
    return {
        "bytes": code.hex(),
        "function_name_part": symbol,
        "expected_verdict": expect,
        "firmware_profile_verdict": actual,
        "ok": bool(found) and actual == expect,
        "sites": [{"address": hex(i["addr"]), "function": function_of(i["addr"]), "asm": i["asm"]}
                  for i in found][:10],
        "sites_total": len(found),
    }


def build_rom(elf_path: Path, objdump: str, binary: Path, calibration_spec: tuple | None = None) -> dict:
    """`calibration_spec` is (hex bytes, symbol, expected verdict) or None for the e_entry default."""
    image = elf_path.read_bytes()
    elf = oracle.Elf64(elf_path)
    sections = executable_sections(image)
    all_insns: list[dict] = []
    for section in sections:
        all_insns += inv.disassemble(elf_path, objdump, section["addr"], section["addr"] + section["size"])
    if not all_insns:
        raise ValueError("no instructions disassembled from the executable sections")
    ranges = function_ranges(elf)
    starts = [r[0] for r in ranges]

    def function_of(address: int) -> str | None:
        return locate_function(ranges, starts, address)

    # Only code inside a sized STT_FUNC symbol is ROM code. The rest of .text is alignment padding (int3) and
    # data placed in the text section; objdump decodes both as instructions, which would be false findings.
    insns = [i for i in all_insns if function_of(i["addr"]) is not None]
    outside = [i for i in all_insns if function_of(i["addr"]) is None]
    by_addr = {i["addr"]: i for i in insns}
    # board_context: how the board runs the profile (CR3, guest FP, RDTSC); bare: step_paged_firmware_probe.
    encodings = sorted({i["bytes"] for i in all_insns})
    verdicts = probe(binary, encodings, board_context=True)
    bare = probe(binary, encodings, board_context=False)
    if calibration_spec is None:
        entry = by_addr.get(elf.e_entry)
        if entry is None:
            raise ValueError("no instruction at e_entry inside a sized function")
        calibration_spec = (entry["bytes"].hex(), (function_of(elf.e_entry) or "").split("::")[-1], "decoded")
    calibration = calibrate(insns, function_of, verdicts, bytes.fromhex(calibration_spec[0]),
                            calibration_spec[1], calibration_spec[2])
    all_addresses = sorted(by_addr)
    seen_entry, info_entry = inv.walk(by_addr, [elf.e_entry])

    def unsupported_rows_of(table: dict) -> list[dict]:
        rows: dict[bytes, dict] = {}
        for insn in insns:
            if table[insn["bytes"]][0] != "unsupported":
                continue
            row = rows.setdefault(insn["bytes"], {
                "bytes": insn["bytes"].hex(), "asm": insn["asm"], "count": 0, "first_address": hex(insn["addr"]),
                "functions": []})
            row["count"] += 1
            function = function_of(insn["addr"])
            if function not in row["functions"] and len(row["functions"]) < 5:
                row["functions"].append(function)
        return sorted(rows.values(), key=lambda r: (-r["count"], r["bytes"]))

    unsupported_rows = unsupported_rows_of(verdicts)
    bare_rows = unsupported_rows_of(bare)
    board_unsupported = {row["bytes"] for row in unsupported_rows}
    # Forms the bare profile rejects but the board context decodes: context artifacts, not ROM defects.
    context_artifacts = [row for row in bare_rows if row["bytes"] not in board_unsupported]

    outside_rows: dict[bytes, dict] = {}
    for insn in outside:
        row = outside_rows.setdefault(insn["bytes"], {"bytes": insn["bytes"].hex(), "asm": insn["asm"], "count": 0,
                                                      "first_address": hex(insn["addr"])})
        row["count"] += 1
    outside_non_int3 = sorted((r for r in outside_rows.values() if r["asm"] != "int3"),
                              key=lambda r: (-r["count"], r["bytes"]))

    forms: dict[str, dict] = {}
    inv.add_forms(forms, insns, "rom", lambda a: a, locate=lambda a: (function_of(a),))
    status_forms: collections.Counter = collections.Counter()
    status_instructions: collections.Counter = collections.Counter()
    uncovered = []
    for row in forms.values():
        row.update(status_for(row))
        status_forms[row["status"]] += 1
        status_instructions[row["status"]] += row["count"]
        if row["status"] in inv.NOT_COVERED:
            uncovered.append({"family": row["family"], "status": row["status"], "count": row["count"],
                              "example_bytes": row["example_bytes"], "example_asm": row["example_asm"],
                              "first_function": row.get("first_function")})
    uncovered.sort(key=lambda r: (-r["count"], r["example_bytes"]))
    sha256, size = oracle.sha256_file(elf_path)
    return {
        "schema": "od-s7-31b-rom-isa-inventory-v2",
        "note": "static estimate over every instruction inside a sized function symbol (dead code included); "
                "not evidence of execution; see the module docstring",
        "elf": {"sha256": sha256, "bytes": size, "entry": hex(elf.e_entry),
                "executable_sections": [{"name": s["name"], "addr": hex(s["addr"]), "size": hex(s["size"])}
                                        for s in sections]},
        "calibration": calibration,
        "linear_instructions_in_text": len(all_insns),
        "instructions_in_functions": len(insns),
        "outside_functions": {
            "note": "padding and data in .text outside any sized STT_FUNC symbol; excluded from every count below",
            "instructions": len(outside),
            "int3_instructions": sum(r["count"] for r in outside_rows.values() if r["asm"] == "int3"),
            "non_int3_instructions": sum(r["count"] for r in outside_non_int3),
            "non_int3_rows": outside_non_int3[:MAX_ROWS],
        },
        "indirect_sites_from_entry": len(info_entry["indirect_sites"]),
        "reachable_from_entry_note": "recursive descent from e_entry over function code; indirect transfers "
                                     "(vectors, tables) are not followed, so this is a lower bound",
        "probe_context": "board: step_paged_firmware_probe_with_guest_fp with CR3, guest FP state and an RDTSC "
                         "source, as od-machine paged_probe.rs; the bare probe is reported separately",
        "firmware_profile_all_functions": summarize_firmware(all_addresses, by_addr, verdicts),
        "firmware_profile_reachable_from_entry": summarize_firmware(sorted(seen_entry), by_addr, verdicts),
        "unsupported_by_firmware_profile_total_encodings": len(unsupported_rows),
        "unsupported_by_firmware_profile": unsupported_rows[:MAX_ROWS],
        "bare_probe_context_artifacts": {
            "note": "encodings the bare step_paged_firmware_probe rejects and the board context decodes "
                    "(mov cr3, FXSAVE/FXRSTOR, RDTSC): false positives of a bare probe",
            "encodings": len(context_artifacts),
            "instructions": sum(r["count"] for r in context_artifacts),
            "rows": context_artifacts[:MAX_ROWS],
        },
        "cpl0_contract_all_functions": {
            "forms_by_status": dict(status_forms),
            "instructions_by_status": dict(status_instructions),
            "status_meaning": "exact and family are covered by CPU_CONTRACT rules; absent, partial and "
                              "not_checked are the forms the contract does not admit or does not name "
                              "(hand-assessed in S4-38 L2, may be stale)",
            "uncovered_forms_total": len(uncovered),
            "uncovered_instructions_total": sum(row["count"] for row in uncovered),
            "uncovered_forms_by_count": uncovered[:MAX_ROWS],
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--vmlinuz", type=Path, help="EFI stub mode: the Ubuntu kernel image (PE)")
    source.add_argument("--rom-elf", type=Path,
                        help="ROM mode: the firmware ELF (tools/build/build_firmware.py output, od-firmware)")
    parser.add_argument("--probe", type=Path, default=DEFAULT_PROBE,
                        help="cargo build --release -p od-isa --example firmware_profile_probe")
    parser.add_argument("--objdump", default=inv.DEFAULT_OBJDUMP)
    parser.add_argument("--out", type=Path, help="default depends on the mode")
    parser.add_argument("--calibration-bytes", help="ROM mode: HEX of the calibration instruction "
                        "(default: the instruction at e_entry)")
    parser.add_argument("--calibration-symbol", help="ROM mode: name component of the function that holds it")
    parser.add_argument("--calibration-expect", choices=("decoded", "unsupported"), default="decoded",
                        help="ROM mode: verdict the probe must give (default decoded)")
    args = parser.parse_args(argv)
    if args.rom_elf is not None:
        return main_rom(args)
    args.out = args.out or DEFAULT_OUT
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


def main_rom(args: argparse.Namespace) -> int:
    if not args.rom_elf.is_file() or not args.probe.is_file():
        print("error: --rom-elf and --probe must exist", file=sys.stderr)
        return 2
    if bool(args.calibration_bytes) != bool(args.calibration_symbol):
        print("error: --calibration-bytes and --calibration-symbol go together", file=sys.stderr)
        return 2
    spec = (args.calibration_bytes, args.calibration_symbol, args.calibration_expect) if args.calibration_bytes else None
    result = build_rom(args.rom_elf, args.objdump, args.probe, spec)
    if not result["calibration"]["ok"]:
        print("error: calibration failed, nothing written: " + json.dumps(result["calibration"]), file=sys.stderr)
        return 3
    text = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if len(text.encode()) > MAX_OUTPUT_BYTES:
        print(f"error: output exceeds {MAX_OUTPUT_BYTES} bytes", file=sys.stderr)
        return 4
    out = args.out or DEFAULT_ROM_OUT
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")
    print(json.dumps({k: result[k] for k in ("calibration", "instructions_in_functions", "firmware_profile_all_functions")},
                     indent=1)[:3000])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
