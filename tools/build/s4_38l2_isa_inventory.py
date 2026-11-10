#!/usr/bin/env python3
"""Static inventory of instruction encodings on the Linux boot path up to C7 (S4-38 L2, part c).

Input for stage-2 planning only; it is NOT evidence of execution. Two code regions are scanned:

* the decompressor outside the payload (`compressed/head_64.S` and the C code reachable by direct
  calls from `.Lrelocated`), taken from the protected-mode part of the bzImage;
* the kernel from `startup_64` to the CPUID(1) in `verify_cpu+0x87` (the `cpuid_divergence` stop):
  `startup_64` up to the `call verify_cpu`, the closure of its direct callees, and `verify_cpu` as a
  linear prefix up to the CPUID.

Disassembly is done by the Xcode `objdump` (LLVM, x86-64 target). Reachability is recursive descent over
the linear disassembly: direct `call`/`jmp`/`jcc` are followed, indirect transfers are counted and not
followed. This is an estimate: code reached only through an indirect transfer is missed (lower bound),
code reached only on branches that are not taken on the target path is included (upper bound). The
number of indirect sites is recorded next to the result.

A form's CPU_CONTRACT status comes from `s4_38l2_contract_status.py` (hand-assessed against
`docs/specs/CPU_CONTRACT.md`, each rule names its evidence). A form no rule covers is reported as
`not_checked`; it is never guessed.

Exit codes: 0 written, 2 usage or input error.
"""

from __future__ import annotations

import argparse
import json
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s4_38l2_oracle as oracle  # noqa: E402
from s4_38l2_contract_status import status_for  # noqa: E402

ROOT = oracle.ROOT
DEFAULT_EVIDENCE = ROOT / "docs/reports/data/stage4/s4-38l2-isa-inventory.json"
DEFAULT_OBJDUMP = "/usr/bin/objdump"
LOAD_ADDRESS = 0x100_0000
CPUID1_OFFSET = 0x87
HEAD_ENTRY = 0x200

PREFIX_BYTES = {
    0x66: "66",
    0x67: "67",
    0xF0: "LOCK",
    0xF2: "REPNE",
    0xF3: "REP",
    0x2E: "CS",
    0x36: "SS",
    0x3E: "DS",
    0x26: "ES",
    0x64: "FS",
    0x65: "GS",
}
ASM_PREFIX_WORDS = {"rep", "repz", "repnz", "repe", "repne", "lock", "cs", "ds", "es", "ss", "fs", "gs", "data16", "notrack", "bnd"}


# --------------------------------------------------------------------------------------
# Disassembly
# --------------------------------------------------------------------------------------


def wrap_as_elf(code: bytes, vaddr: int) -> bytes:
    """Wrap raw x86-64 code in a minimal ET_EXEC ELF so that objdump can disassemble it."""
    shstrtab = b"\x00.text\x00.shstrtab\x00"
    ehsize, phsize, shsize = 64, 56, 64
    code_off = ehsize + phsize
    str_off = code_off + len(code)
    sh_off = (str_off + len(shstrtab) + 7) & ~7
    ehdr = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8)
    ehdr += struct.pack("<HHIQQQIHHHHHH", 2, 62, 1, vaddr, ehsize, sh_off, 0, ehsize, phsize, 1, shsize, 3, 2)
    phdr = struct.pack("<IIQQQQQQ", 1, 5, code_off, vaddr, vaddr, len(code), len(code), 0x1000)
    text_sh = struct.pack("<IIQQQQIIQQ", 1, 1, 6, vaddr, code_off, len(code), 0, 0, 1, 0)
    str_sh = struct.pack("<IIQQQQIIQQ", 7, 3, 0, 0, str_off, len(shstrtab), 0, 0, 1, 0)
    body = ehdr + phdr + code + shstrtab
    body += bytes(sh_off - len(body))
    return body + bytes(shsize) + text_sh + str_sh


LINE = re.compile(r"^\s*([0-9a-f]+):\s*(.*)$")


def parse_objdump(text: str) -> list[dict]:
    """Parse `objdump -d` lines: ` addr: <bytes>\t<mnemonic operands>`; long instructions may wrap."""
    insns: list[dict] = []
    for raw in text.splitlines():
        match = LINE.match(raw)
        if not match:
            continue
        rest = match.group(2)
        raw_bytes, _, asm = rest.partition("\t")
        try:
            data = bytes.fromhex(raw_bytes.replace(" ", ""))
        except ValueError:
            continue
        asm = asm.replace("\t", " ").split("#")[0].strip()
        if insns and not asm:
            insns[-1]["bytes"] += data  # continuation line of a long instruction
            continue
        insns.append({"addr": int(match.group(1), 16), "bytes": data, "asm": asm})
    return insns


def disassemble(elf_path: Path, objdump: str, start: int, stop: int) -> list[dict]:
    proc = subprocess.run(
        [objdump, "-d", "-M", "intel", f"--start-address={start:#x}", f"--stop-address={stop:#x}", str(elf_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise oracle.OracleError(f"objdump failed: {proc.stderr.strip()[:200]}")
    return parse_objdump(proc.stdout)


# --------------------------------------------------------------------------------------
# Form classification
# --------------------------------------------------------------------------------------

REG64 = {"rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp"} | {f"r{i}" for i in range(8, 16)}
REG32 = {"eax", "ebx", "ecx", "edx", "esi", "edi", "ebp", "esp"} | {f"r{i}d" for i in range(8, 16)}
REG16 = {"ax", "bx", "cx", "dx", "si", "di", "bp", "sp"} | {f"r{i}w" for i in range(8, 16)}
REG8 = {"al", "bl", "cl", "dl", "ah", "bh", "ch", "dh", "sil", "dil", "bpl", "spl"} | {f"r{i}b" for i in range(8, 16)}
SEG = {"es", "cs", "ss", "ds", "fs", "gs"}


def split_asm(asm: str) -> tuple[list[str], str, list[str]]:
    tokens = asm.split()
    prefixes: list[str] = []
    while tokens and tokens[0] in ASM_PREFIX_WORDS and len(tokens) > 1:
        prefixes.append(tokens.pop(0))
    if not tokens:
        return prefixes, "?", []
    mnemonic = tokens[0]
    rest = " ".join(tokens[1:])
    operands = [o.strip() for o in rest.split(",")] if rest else []
    return prefixes, mnemonic, operands


def operand_class(op: str) -> str:
    low = op.lower()
    if "[" in low:
        size = re.match(r"^(byte|word|dword|qword|xmmword|tbyte|fword|zmmword|ymmword)\s+ptr", low)
        inner = low[low.index("[") + 1 : low.index("]")]
        seg = re.match(r"^(?:\w+\s+ptr\s+)?(es|cs|ss|ds|fs|gs):", low)
        regs = re.findall(r"\b(?:r[a-z0-9]+|e[a-z]{2})\b", inner)
        if "rip" in inner:
            shape = "rip"
        elif "*" in inner:
            shape = "sib"
        elif not regs:
            shape = "abs"
        elif re.search(r"[+-]\s*0x[0-9a-f]+|[+-]\s*\d+", inner):
            shape = "base+disp"
        else:
            shape = "base"
        label = f"m{('(' + size.group(1) + ')') if size else ''}[{shape}]"
        if seg:
            label = f"{seg.group(1)}:" + label
        return label
    if low in REG64:
        return "r64"
    if low in REG32:
        return "r32"
    if low in REG16:
        return "r16"
    if low in REG8:
        return "r8"
    if low in SEG:
        return "sreg"
    if re.match(r"^cr\d+$", low):
        return "creg"
    if re.match(r"^(xmm|ymm|zmm)\d+$", low):
        return "xmm"
    if re.match(r"^st(\(\d\))?$", low):
        return "st"
    if re.match(r"^-?(0x[0-9a-f]+|\d+)$", low):
        return "imm"
    return "other"


def encoding_parts(data: bytes) -> dict:
    pos = 0
    prefixes: list[str] = []
    while pos < len(data) and data[pos] in PREFIX_BYTES:
        prefixes.append(PREFIX_BYTES[data[pos]])
        pos += 1
    rex = 0
    if pos < len(data) and 0x40 <= data[pos] <= 0x4F:
        rex = data[pos]
        pos += 1
    if data[pos] in (0xC4, 0xC5, 0x62):
        return {"prefixes": prefixes, "rex": rex, "opcode": "vex", "vex": True}
    if data[pos] == 0x0F:
        length = 3 if pos + 1 < len(data) and data[pos + 1] in (0x38, 0x3A) else 2
    else:
        length = 1
    return {"prefixes": prefixes, "rex": rex, "opcode": data[pos : pos + length].hex(), "vex": False}


def describe(insn: dict) -> dict:
    enc = encoding_parts(insn["bytes"])
    asm_prefixes, mnemonic, operands = split_asm(insn["asm"])
    classes = [operand_class(o) for o in operands]
    is_branch = mnemonic.startswith(("j", "call", "loop"))
    if is_branch and operands and re.match(r"^0x[0-9a-f]+", operands[0].lower()):
        classes = ["rel"]
    if enc["vex"]:
        enc["opcode"] = f"vex-{mnemonic}"
    modrm_reg = None
    if enc["opcode"] in GROUP_OPCODES:
        modrm_reg = mnemonic  # the mnemonic identifies the /digit
    simple = ["m" if "[" in c else c for c in classes]
    family = " ".join(
        part
        for part in (
            "+".join(enc["prefixes"]),
            "W" if enc["rex"] & 8 else "",
            mnemonic,
            ",".join(simple),
        )
        if part
    )
    key_parts = []
    if enc["prefixes"]:
        key_parts.append("+".join(enc["prefixes"]))
    key_parts.append("REX.W" if enc["rex"] & 8 else ("REX" if enc["rex"] else ""))
    key_parts = [p for p in key_parts if p]
    key_parts.append(enc["opcode"])
    sig = f"{mnemonic} {', '.join(classes)}".strip()
    if mnemonic in EXACT_ONLY_MNEMONICS or "sreg" in classes[:1]:
        sig += f" ({insn['bytes'].hex()})"  # contract slices admit single byte strings: keep them apart
    return {
        "key": f"{' '.join(key_parts)} | {sig}",
        "family": family,
        "mnemonic": mnemonic,
        "operands": classes,
        "opcode": enc["opcode"],
        "prefixes": enc["prefixes"],
        "rex_w": bool(enc["rex"] & 8),
        "rex": bool(enc["rex"]),
        "vex": enc["vex"],
        "asm_prefixes": asm_prefixes,
        "modrm_reg": modrm_reg,
    }


EXACT_ONLY_MNEMONICS = {"lgdt", "lidt"}

GROUP_OPCODES = {"80", "81", "83", "8f", "c0", "c1", "c6", "c7", "d0", "d1", "d2", "d3", "f6", "f7", "fe", "ff", "0f00", "0f01", "0f18", "0f1f", "0fae", "0fba", "0fc7"}


# --------------------------------------------------------------------------------------
# Reachability
# --------------------------------------------------------------------------------------


def direct_target(asm: str) -> int | None:
    _, mnemonic, operands = split_asm(asm)
    if not operands or not (mnemonic.startswith("j") or mnemonic.startswith("call") or mnemonic.startswith("loop")):
        return None
    match = re.match(r"^0x([0-9a-f]+)", operands[0].lower())
    return int(match.group(1), 16) if match else None


def mnemonic_of(asm: str) -> str:
    return split_asm(asm)[1]


def terminates(asm: str) -> bool:
    return mnemonic_of(asm) in {"jmp", "ret", "retq", "lret", "lretq", "iret", "iretq", "hlt", "ud2", "sysret", "sysretq"}


def walk(
    insns: dict[int, dict],
    seeds: list[int],
    stop_after: int | None = None,
    decisions: dict[int, bool] | None = None,
    noreturn: set[int] | None = None,
) -> tuple[set[int], dict]:
    """Recursive descent. `stop_after`: the walk does not continue past this instruction address.

    `decisions` maps the address of a conditional jump to "taken" (True) or "not taken" (False): only
    that successor is followed. It is used where the target profile fixes the outcome.
    """
    seen: set[int] = set()
    stack = list(seeds)
    info = {"indirect_sites": [], "outside_targets": []}
    while stack:
        addr = stack.pop()
        while addr in insns and addr not in seen:
            insn = insns[addr]
            seen.add(addr)
            if stop_after is not None and addr == stop_after:
                break
            asm = insn["asm"]
            mnemonic = mnemonic_of(asm)
            target = direct_target(asm)
            if decisions is not None and addr in decisions and mnemonic != "jmp":
                if decisions[addr]:
                    if target in insns:
                        stack.append(target)
                    break
                addr += len(insn["bytes"])
                continue
            if target is not None:
                if addr in PRUNED_CALLS:
                    pass
                elif target in insns:
                    stack.append(target)
                else:
                    info["outside_targets"].append((addr, target, mnemonic))
            elif mnemonic in ("jmp", "call"):
                info["indirect_sites"].append(addr)
            if terminates(asm):
                break
            if noreturn and mnemonic == "call" and target in noreturn:
                break  # padding after a call that never returns is not executed
            addr += len(insn["bytes"])
    return seen, info


def find_noreturn(insns: list[dict]) -> set[int]:
    """Call targets whose straight-line body reaches `hlt` before any ret/jmp (the decompressor `error()` halts)."""
    index = {insn["addr"]: n for n, insn in enumerate(insns)}
    targets = {direct_target(i["asm"]) for i in insns if mnemonic_of(i["asm"]) == "call"}
    result: set[int] = set()
    for target in targets:
        n = index.get(target)
        if n is None:
            continue
        for offset, insn in enumerate(insns[n : n + 40]):
            name = mnemonic_of(insn["asm"])
            if name in ("ret", "retq", "jmp") or (offset and name == "endbr64"):
                break  # the body ended (or a new function began) without reaching hlt
            if name == "hlt":
                result.add(target)
                break
    return result


# --------------------------------------------------------------------------------------
# Regions
# --------------------------------------------------------------------------------------


def decompressor_region(image: bytes, header: dict, objdump: str, work: Path) -> dict:
    pm = image[header["pm_offset"] :]
    reloc = oracle.relocated_copy_address(pm, header, header["pref_address"])
    head_end = header["payload_offset"]
    text_start = reloc["rva_Lrelocated_imm32"]
    head = pm[HEAD_ENTRY:head_end]
    text = pm[text_start:]
    elf_head = work / "zo-head.elf"
    elf_text = work / "zo-text.elf"
    elf_head.write_bytes(wrap_as_elf(head, LOAD_ADDRESS + HEAD_ENTRY))
    elf_text.write_bytes(wrap_as_elf(text, LOAD_ADDRESS + text_start))
    head_insns = disassemble(elf_head, objdump, LOAD_ADDRESS + HEAD_ENTRY, LOAD_ADDRESS + head_end)
    text_insns = disassemble(elf_text, objdump, LOAD_ADDRESS + text_start, LOAD_ADDRESS + len(pm))
    head_map = {i["addr"]: i for i in head_insns}
    text_map = {i["addr"]: i for i in text_insns}
    noreturn = find_noreturn(text_insns)
    seen_head, info_head = walk(head_map, [LOAD_ADDRESS + HEAD_ENTRY])
    seen_text, info_text = walk(text_map, [LOAD_ADDRESS + text_start], noreturn=noreturn)
    return {
        "relocated_copy": reloc,
        "head_range": [LOAD_ADDRESS + HEAD_ENTRY, LOAD_ADDRESS + head_end],
        "head_last_insn": head_insns[-1]["asm"] if head_insns else None,
        "head": [head_map[a] for a in sorted(seen_head)],
        "text": [text_map[a] for a in sorted(seen_text)],
        "text_start": LOAD_ADDRESS + text_start,
        "info": {
            "head_indirect_sites": [hex(a) for a in info_head["indirect_sites"]],
            "text_indirect_sites": [hex(a) for a in info_text["indirect_sites"]],
            "text_outside_targets": [(hex(a), hex(t), m) for a, t, m in info_text["outside_targets"]][:20],
            "text_outside_target_count": len(info_text["outside_targets"]),
            "noreturn_functions": sorted(hex(a) for a in noreturn),
        },
        "text_linear_instruction_count": len(text_insns),
    }


def kernel_region(stage_dir: Path, objdump: str, debug: oracle.Elf64) -> dict:
    vmlinux = stage_dir / oracle.VMLINUX
    symbols = debug.symbols(None)
    funcs = sorted(
        (s["value"], s["size"], name)
        for name, entries in symbols.items()
        for s in entries
        if s["type"] in (0, 2) and s["size"] > 0
    )
    by_name = {name: (value, size) for value, size, name in funcs}
    starts = [f[0] for f in funcs]

    def locate(addr: int) -> tuple[str, int, int] | None:
        import bisect

        i = bisect.bisect_right(starts, addr) - 1
        if i >= 0 and funcs[i][0] <= addr < funcs[i][0] + funcs[i][1]:
            return funcs[i][2], funcs[i][0], funcs[i][1]
        return None

    startup_va, startup_size = by_name["startup_64"]
    verify_va, verify_size = by_name["verify_cpu"]
    cpuid1_va = verify_va + CPUID1_OFFSET
    cache: dict[str, dict[int, dict]] = {}

    def function_insns(name: str) -> dict[int, dict]:
        if name not in cache:
            va, size = by_name[name]
            cache[name] = {i["addr"]: i for i in disassemble(vmlinux, objdump, va, va + size)}
        return cache[name]

    path: dict[int, dict] = {}
    functions_in_path: dict[str, str] = {}
    unresolved_calls: list[dict] = []
    indirect_sites: list[int] = []

    # startup_64 up to and including `call verify_cpu`.
    startup = function_insns("startup_64")
    call_verify = next((a for a, i in sorted(startup.items()) if direct_target(i["asm"]) == verify_va and mnemonic_of(i["asm"]) == "call"), None)
    if call_verify is None:
        raise oracle.OracleError("call verify_cpu not found in startup_64")
    seen, info = walk(startup, [startup_va], stop_after=call_verify)
    indirect_sites += info["indirect_sites"]
    for a in seen:
        path[a] = startup[a]
    functions_in_path["startup_64"] = "startup_64 up to `call verify_cpu`"
    work = [(a, startup[a]) for a in sorted(seen)]
    pending: list[str] = []
    for _, insn in work:
        if mnemonic_of(insn["asm"]) == "call":
            target = direct_target(insn["asm"])
            found = locate(target) if target is not None else None
            if target is None:
                continue
            if found is None:
                unresolved_calls.append({"at": insn["addr"], "target": target})
            elif found[0] != "verify_cpu":
                pending.append(found[0])
    while pending:
        name = pending.pop()
        if name in functions_in_path:
            continue
        functions_in_path[name] = "callee closure (whole function)"
        insns = function_insns(name)
        decisions = None
        if name == "__pi_sme_enable":
            # CPUID(0x80000000).EAX = 0x80000008 on the target profile: `cmp eax,0x8000001e; jbe` is taken.
            decisions = {by_name[name][0] + 0x20: True}
        seen, info = walk(insns, [by_name[name][0]], decisions=decisions)
        indirect_sites += info["indirect_sites"]
        for a in seen:
            path[a] = insns[a]
            insn = insns[a]
            if mnemonic_of(insn["asm"]) == "call":
                target = direct_target(insn["asm"])
                if target is None:
                    continue
                found = locate(target)
                if found is None:
                    unresolved_calls.append({"at": insn["addr"], "target": target})
                elif found[0] not in functions_in_path and found[0] != "verify_cpu":
                    pending.append(found[0])
    # verify_cpu: linear prefix up to CPUID(1).
    verify = function_insns("verify_cpu")
    # Target vendor (DiagnosticV1: "OpenDesktop ") is neither AuthenticAMD nor GenuineIntel, and
    # CPUID(0).EAX = 1: jb at +0x0e not taken, AMD compare jne at +0x1d taken, Intel compare jne at +0x3b taken.
    decisions = {verify_va + 0x0E: False, verify_va + 0x1D: True, verify_va + 0x3B: True}
    seen, info = walk(verify, [verify_va], stop_after=cpuid1_va, decisions=decisions)
    if cpuid1_va not in seen:
        raise oracle.OracleError("CPUID(1) at verify_cpu+0x87 is not on the decided verify_cpu path")
    for a in seen:
        path[a] = verify[a]
    functions_in_path["verify_cpu"] = (
        "decided path up to CPUID(1) at verify_cpu+0x87 for a vendor that is neither AuthenticAMD nor GenuineIntel; "
        "GenuineIntel would execute another CPUID(1) at verify_cpu+0x52 plus RDMSR 0x1a0/BTR/WRMSR"
    )
    return {
        "insns": [path[a] for a in sorted(path)],
        "functions": functions_in_path,
        "locate": locate,
        "unresolved_calls": unresolved_calls,
        "indirect_sites": indirect_sites,
        "cpuid1_va": cpuid1_va,
        "startup_va": startup_va,
        "verify_va": verify_va,
    }


# --------------------------------------------------------------------------------------
# Inventory
# --------------------------------------------------------------------------------------


STATUS_ORDER = {"exact": 0, "family": 1, "not_checked": 2, "partial": 3, "absent": 4}
KERNEL_PA_DELTA = 0xFFFFFFFF80000000
SEV_NAME = re.compile(r"sev|snp|svsm|rmpadjust|pvalidate", re.I)

# Call sites of the decompressor that are cut from the reachability walk, with the reason. Each entry
# was read in the disassembly; the cut removes code that is not executed on the target path.
PRUNED_CALLS: dict[int, str] = {
    0x2067942: (
        "call has_cpuflag from the GHCB shutdown helper at 0x2067930: the helper starts with "
        "`cmp qword [rip+..],0; je ret` (boot_ghcb == NULL without SEV-ES), so get_cpuflags (CPUID 1, 7, 0x80000001) "
        "is not executed on a non-SEV guest"
    ),
}


def cpuid_leaf(window: list[dict]) -> str:
    """Leaf constant loaded into EAX shortly before a CPUID (heuristic: last write to eax in the window)."""
    for insn in reversed(window):
        _, mnemonic, operands = split_asm(insn["asm"])
        if len(operands) == 2 and operands[0] == "eax":
            if mnemonic == "xor" and operands[1] == "eax":
                return "0x0"
            if mnemonic == "mov" and re.match(r"^0x[0-9a-f]+$", operands[1]):
                return operands[1]
            return "dynamic"
    return "dynamic"


def cpuid_sites(insns: list[dict], region: str, runtime_of) -> list[dict]:
    out = []
    for index, insn in enumerate(insns):
        if mnemonic_of(insn["asm"]) == "cpuid":
            out.append(
                {
                    "region": region,
                    "address": insn["addr"],
                    "runtime_address": runtime_of(insn["addr"]),
                    "leaf": cpuid_leaf(insns[max(0, index - 6) : index]),
                }
            )
    return out


def add_forms(forms: dict[str, dict], insns: list[dict], region: str, runtime_of, locate=None) -> None:
    for insn in insns:
        info = describe(insn)
        function = (locate(insn["addr"]) or (None,))[0] if locate is not None else None
        if info["mnemonic"] in ("wrmsr", "rdmsr"):
            # The MSR index is dynamic: keep the use sites apart (the kernel startup_64 WRMSR is GS_BASE).
            info["key"] += f" @{function or region}"
        entry = forms.get(info["key"])
        if entry is None:
            entry = {
                "key": info["key"],
                "family": info["family"],
                "mnemonic": info["mnemonic"],
                "operands": info["operands"],
                "opcode": info["opcode"],
                "prefixes": info["prefixes"],
                "rex": info["rex"],
                "rex_w": info["rex_w"],
                "region": region,
                "first_address": runtime_of(insn["addr"]),
                "example_bytes": insn["bytes"].hex(),
                "example_asm": insn["asm"],
                "count": 0,
                "regions": [],
                "path_classes": [],
            }
            if function is not None:
                entry["first_function"] = function
            forms[info["key"]] = entry
        entry["count"] += 1
        if region not in entry["regions"]:
            entry["regions"].append(region)
        if region.startswith("decompressor_head"):
            cls = "head_asm"
        elif region.startswith("decompressor_text"):
            cls = "decompressor_c"
        elif function is not None and SEV_NAME.search(function):
            cls = "kernel_sev_helper"
        else:
            cls = "kernel_asm"
        if cls not in entry["path_classes"]:
            entry["path_classes"].append(cls)


def build_inventory(stage_dir: Path, objdump: str, work: Path) -> dict:
    image = (stage_dir / oracle.VMLINUZ).read_bytes()
    header = oracle.parse_setup_header(image)
    debug = oracle.Elf64(stage_dir / oracle.VMLINUX_DEBUG)
    zo = decompressor_region(image, header, objdump, work)
    kernel = kernel_region(stage_dir, objdump, debug)
    rbx = zo["relocated_copy"]["rbx_relocated_copy"]
    forms: dict[str, dict] = {}
    # Execution order: head (load address, before the copy), text (relocated copy), kernel (physical addresses).
    add_forms(forms, zo["head"], "decompressor_head_64.S_startup_64", lambda a: a)
    add_forms(forms, zo["text"], "decompressor_text_after_relocation", lambda a: rbx + (a - LOAD_ADDRESS))
    add_forms(forms, kernel["insns"], "kernel_startup_64_to_cpuid1", lambda a: a - KERNEL_PA_DELTA, kernel["locate"])
    rows = list(forms.values())
    for row in rows:
        # Unconditional: executed by the target path itself (boot assembly), not merely reachable C code.
        row["unconditional"] = bool(set(row["path_classes"]) & {"head_asm", "kernel_asm"})
        row.update(status_for(row))
    cpuids = (
        cpuid_sites(zo["head"], "decompressor_head", lambda a: a)
        + cpuid_sites(zo["text"], "decompressor_text", lambda a: rbx + (a - LOAD_ADDRESS))
        + cpuid_sites(kernel["insns"], "kernel", lambda a: a - KERNEL_PA_DELTA)
    )
    families: dict[str, dict] = {}
    for row in rows:
        fam = families.setdefault(
            row["family"],
            {
                "family": row["family"],
                "status": row["status"],
                "evidence": row["evidence"],
                "forms": 0,
                "first_address": row["first_address"],
                "region": row["region"],
                "example_asm": row["example_asm"],
                "example_bytes": row["example_bytes"],
                "unconditional": False,
            },
        )
        fam["forms"] += 1
        fam["unconditional"] = fam["unconditional"] or row["unconditional"]
        if STATUS_ORDER[row["status"]] > STATUS_ORDER[fam["status"]]:
            fam["status"], fam["evidence"] = row["status"], row["evidence"]
    return {
        "zo": zo,
        "kernel": kernel,
        "rows": rows,
        "families": list(families.values()),
        "cpuid_sites": cpuids,
        "counts": {
            "decompressor_head_instructions": len(zo["head"]),
            "decompressor_text_reachable_instructions": len(zo["text"]),
            "decompressor_text_linear_instructions": zo["text_linear_instruction_count"],
            "kernel_instructions": len(kernel["insns"]),
        },
    }


def summarise(rows: list[dict]) -> dict:
    out: dict[str, int] = {}
    for row in rows:
        out[row["status"]] = out.get(row["status"], 0) + 1
    return out


NOT_COVERED = ("absent", "partial", "not_checked")

# Hypotheses of STAGE_4.md S4-38L7 checked against the inventory: (label, predicate on a form row).
L7_HYPOTHESES = [
    ("MOV SS (null selector)", lambda r: r["example_bytes"] == "8ed0"),
    ("MOV FS null on CPL0", lambda r: r["example_bytes"] == "8ee0"),
    ("MOV GS null on CPL0", lambda r: r["example_bytes"] == "8ee8"),
    ("LGDT in a general addressing form (decompressor LGDT [RAX])", lambda r: r["mnemonic"] == "lgdt" and r["example_bytes"] == "0f0110"),
    ("LGDT [RBP-10] (kernel)", lambda r: r["mnemonic"] == "lgdt" and r["example_bytes"] == "0f0155f6"),
    ("LIDT [RSP+6] (kernel)", lambda r: r["mnemonic"] == "lidt" and r["example_bytes"] == "0f015c2406"),
    ("LRETQ", lambda r: r["mnemonic"] == "retfq"),
    ("PUSHFQ", lambda r: r["mnemonic"] == "pushfq"),
    ("POPFQ on CPL0", lambda r: r["mnemonic"] == "popfq"),
    ("MOV r64,CR4 / MOV CR4,r64", lambda r: r["opcode"] in ("0f20", "0f22")),
    ("STD", lambda r: r["mnemonic"] == "std"),
    ("REP MOVSQ", lambda r: r["mnemonic"] == "movsq"),
    ("F3 0F BC (REP BSF / TZCNT)", lambda r: r["opcode"] == "0fbc"),
    ("WRMSR GS_BASE at kernel startup_64", lambda r: r["mnemonic"] == "wrmsr" and r.get("first_function") == "startup_64"),
    ("MOV CR3 / MOV CR2 reads on the path", lambda r: r["opcode"] in ("0f20", "0f22") and r["example_asm"].replace(" ", "").endswith(("cr3", "cr2"))),
]


def l7_check(rows: list[dict]) -> list[dict]:
    out = []
    for label, predicate in L7_HYPOTHESES:
        found = [r for r in rows if predicate(r)]
        out.append(
            {
                "hypothesis": label,
                "found_on_path": bool(found),
                "status": found[0]["status"] if found else None,
                "first_address": found[0]["first_address"] if found else None,
                "unconditional": any(r["unconditional"] for r in found) if found else None,
            }
        )
    return out


def run(args: argparse.Namespace) -> int:
    def go(work: Path) -> int:
        result = build_inventory(args.stage_dir, args.objdump, work)
        rows, families = result["rows"], result["families"]
        absent_forms = [r for r in rows if r["status"] == "absent"]
        absent_families = [f for f in families if f["status"] == "absent"]
        version = subprocess.run([args.objdump, "--version"], capture_output=True, text=True, check=False).stdout.splitlines()
        evidence = {
            "slice": "S4-38L2",
            "tool": "tools/build/s4_38l2_isa_inventory.py",
            "kind": "static_inventory_not_execution_evidence",
            "disassembler": version[0] if version else "unknown",
            "method_limits": [
                "Recursive descent over linear disassembly; indirect jmp/call are not followed (lower bound).",
                "Branches not taken on the target path are followed (upper bound); verify_cpu is a linear prefix up to CPUID(1). Cut call sites are listed in pruned_calls.",
                "The decompressor has no symbols in vmlinuz: functions are unnamed; first_address of a decompressor head form is the load address (0x1000000-based), of a text form rbx + rva (relocated copy); kernel forms use physical addresses (va - 0xffffffff80000000).",
                "32-bit startup_32 and the 32-bit trampoline are not on the 64-bit-entry path (entry 0x200) and are not scanned.",
                "A form is keyed by prefixes, REX.W, opcode, mnemonic and operand classes (memory operands by size and addressing shape); a family drops the memory shape.",
                "Statuses are hand-assessed against CPU_CONTRACT.md in s4_38l2_contract_status.py; `not_checked` means no rule covers the form.",
            ],
            "cpuid1": {"va": result["kernel"]["cpuid1_va"], "pa": result["kernel"]["cpuid1_va"] - KERNEL_PA_DELTA},
            "relocated_copy": result["zo"]["relocated_copy"],
            "counts": result["counts"],
            "reachability_info": result["zo"]["info"],
            "pruned_calls": {hex(k): v for k, v in PRUNED_CALLS.items()},
            "kernel_functions_in_path": result["kernel"]["functions"],
            "kernel_unresolved_calls": [{"at": hex(c["at"]), "target": hex(c["target"])} for c in result["kernel"]["unresolved_calls"]],
            "kernel_indirect_sites": [hex(a) for a in result["kernel"]["indirect_sites"]],
            "cpuid_sites": result["cpuid_sites"],
            "form_count": len(rows),
            "family_count": len(families),
            "status_counts_forms": summarise(rows),
            "status_counts_families": summarise(families),
            "absent_forms": len(absent_forms),
            "absent_families": len(absent_families),
            "unconditional_path": {
                "forms": summarise([r for r in rows if r["unconditional"]]),
                "families": summarise([f for f in families if f["unconditional"]]),
            },
            "decompressor_c_only": {
                "forms": summarise([r for r in rows if not r["unconditional"]]),
                "families": summarise([f for f in families if not f["unconditional"]]),
            },
            "summary": {
                "unconditional_not_covered_forms": [
                    {k: r[k] for k in ("status", "key", "first_address", "example_bytes", "example_asm", "path_classes")}
                    for r in rows
                    if r["unconditional"] and r["status"] in NOT_COVERED
                ],
                "decompressor_c_not_covered_forms": len([r for r in rows if not r["unconditional"] and r["status"] in NOT_COVERED]),
                "all_not_covered_forms": len([r for r in rows if r["status"] in NOT_COVERED]),
                "all_absent_forms": len([r for r in rows if r["status"] == "absent"]),
                "all_partial_forms": len([r for r in rows if r["status"] == "partial"]),
                "all_not_checked_forms": len([r for r in rows if r["status"] == "not_checked"]),
            },
            "l7_hypotheses_check": l7_check(rows),
            "notes": [
                "unconditional = the form occurs in boot assembly executed by the path itself (head_64.S startup_64, kernel startup_64..verify_cpu and the helpers it calls outside SEV-named functions).",
                "decompressor C code is an upper bound: reachable by direct calls, not proven executed. `endbr64` (every function entry) and `REP BSF`/TZCNT (zstd bit scans, compiled without BMI) are very likely executed there.",
                "BMI1/BMI2 VEX forms, PREFETCH, CLFLUSH, RDRAND, RDTSC (KASLR), SEV/TDX instructions sit behind CPUID/boot-parameter checks that are false on the target profile or behind nokaslr; they are listed as reachable, not as expected stops.",
                "verify_cpu: the path to CPUID(1) at +0x87 assumes a vendor that is neither AuthenticAMD nor GenuineIntel (DiagnosticV1 vendor). With GenuineIntel the first CPUID(1) is at verify_cpu+0x52 (0x139c252), followed by RDMSR 0x1a0 / WRMSR.",
                "Decompressor CPUID with leaf 1 / 0x80000001 / 7 (get_cpuflags) is called only through has_cpuflag: from kaslr_get_random_long (arch/x86/lib/kaslr.c:64,73; reached from compressed/kaslr.c:536,852 only after choose_random_location passes the nokaslr return at kaslr.c:869-871) and from the boot_ghcb helper (cut, see pruned_calls). It is reachable in this walk but not executed on the nokaslr non-SEV path (static source reading).",
                "Decompressor CPUID on the path by source: leaf 0 and (only if EAX(0) >= 7) leaf 7 in pgtable_64.c:122; 0x80000000/0x8000001f in sev_enable; 0x21 in early_tdx_detect; leaf 0 in ZSTD_initDCtx. With the DiagnosticV1 answers (EAX(0)=1) leaf 7 is not issued.",
                "__pi_sme_enable is walked with CPUID(0x80000000).EAX = 0x80000008 (DiagnosticV1): the early return at +0x20 is taken; its callee __pi_snp_init is included as a whole-function closure (SEV helpers are tagged kernel_sev_helper).",
            ],
            "families": families,
            "forms": rows,
        }
        if args.dump_forms:
            lines = [
                f"{f['status']:11s} {f['first_address']:#x} {f['family']:34s} [{f['example_bytes']}  {f['example_asm']}]"
                for f in families
            ]
            args.dump_forms.write_text("\n".join(lines) + "\n")
        args.evidence.parent.mkdir(parents=True, exist_ok=True)
        args.evidence.write_text(json.dumps(evidence, indent=2) + "\n")
        print(f"forms={len(rows)} families={len(families)} status_forms={summarise(rows)} status_families={summarise(families)}")
        print(f"evidence: {args.evidence}")
        return 0

    if args.work_dir:
        resolved = args.work_dir.resolve()
        if ROOT in resolved.parents:
            print("work dir must be outside the repository", file=sys.stderr)
            return 2
        resolved.mkdir(parents=True, exist_ok=True)
        return go(resolved)
    with tempfile.TemporaryDirectory(prefix="s4-38l2-isa-") as tmp:
        return go(Path(tmp))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--stage-dir", type=Path, default=oracle.DEFAULT_STAGE_DIR)
    parser.add_argument("--objdump", default=DEFAULT_OBJDUMP)
    parser.add_argument("--work-dir", type=Path, help="temporary directory outside Git")
    parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    parser.add_argument("--dump-forms", type=Path, help="write a plain-text family list (debug)")
    args = parser.parse_args(argv)
    if not Path(args.objdump).is_file():
        print(f"objdump not found: {args.objdump}", file=sys.stderr)
        return 2
    try:
        return run(args)
    except oracle.OracleError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
