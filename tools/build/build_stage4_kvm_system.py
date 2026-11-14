#!/usr/bin/env python3
"""Build and run the S4-34c/d/e freestanding system fixtures on Mini KVM."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import struct
import sys
from pathlib import Path
from typing import Any

import build_stage4_kvm_method as base

ROOT = Path(__file__).resolve().parents[2]
FIXTURE_DIR = ROOT / "tests/fixtures/guest/stage4/kvm_system"
DEFAULT_OUTPUT = ROOT / "docs/reports/data/stage4/s4-34cde-kvm-system.json"
DEFAULT_WORKDIR = ROOT / "work/stage4/kvm_system"
RESULT_GPA = 0x18000
RESULT_SIZE = 512
MAGIC = 0x314D564B57483453
COMPLETE = 0x434F4D50
REPEATS = 3
EXPECTED_WORDS = {
    "paging_101": 7, "paging_102": 7, "paging_103": 8, "paging_104": 9,
    "paging_105": 7, "paging_106": 10, "paging_107": 7, "paging_108": 6,
    "control": 13, "tables_roundtrip": 25, "int_iret_cpl0": 12,
    "cpl3_roundtrip": 7, "cpl3_privileged_gp": 8, "kvm_injected_interrupt": 7,
}

# Case name maps to source, define, ID, and expected guest event count.
CASES: dict[str, dict[str, Any]] = {
    **{
        f"paging_{case_id}": {
            "source": "paging.S", "define": f"CASE_ID={case_id}", "id": case_id,
            "events": 1 if case_id in (103, 104, 106) else 0,
        }
        for case_id in range(101, 109)
    },
    "control": {"source": "control.S", "define": None, "id": 201, "events": 0},
    "tables_roundtrip": {"source": "descriptors.S", "define": "KVM_SYSTEM_CASE=0x21", "id": 0x21, "events": 0},
    "int_iret_cpl0": {"source": "descriptors.S", "define": "KVM_SYSTEM_CASE=0x22", "id": 0x22, "events": 1},
    "cpl3_roundtrip": {"source": "privilege.S", "define": "KVM_SYSTEM_CASE=0x23", "id": 0x23, "events": 2},
    "cpl3_privileged_gp": {"source": "privilege.S", "define": "KVM_SYSTEM_CASE=0x24", "id": 0x24, "events": 2},
    "kvm_injected_interrupt": {"source": "interrupt.S", "define": None, "id": 0x31, "events": 1, "inject": 0x41},
}


class BuildError(Exception):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def compile_image(case: str, workdir: Path) -> tuple[bytes, dict[str, Any]]:
    if case not in CASES:
        raise BuildError(f"unknown case: {case}")
    spec = CASES[case]
    clang = base.find_tool("STAGE4_CLANG", ["clang"], [])
    ld = base.find_tool("STAGE4_LD_LLD", ["ld.lld"], ["gcc-ld/ld.lld"])
    objcopy = base.find_tool("STAGE4_OBJCOPY", ["llvm-objcopy"], ["llvm-objcopy", "rust-objcopy"])
    nm = base.find_tool("STAGE4_NM", ["llvm-nm", "nm"], ["llvm-nm", "rust-nm"])
    workdir.mkdir(parents=True, exist_ok=True)
    stem = case
    obj, elf, raw = (workdir / f"{stem}.{ext}" for ext in ("o", "elf", "bin"))
    command = [str(clang), "--target=x86_64-unknown-none-elf", "-c", "-ffreestanding", "-nostdlib"]
    if spec["define"]:
        command.append(f"-D{spec['define']}")
    command += [str(FIXTURE_DIR / spec["source"]), "-o", str(obj)]
    base.run_local(command)
    env = base.tool_env(ld, objcopy)
    base.run_local([str(ld), "-T", str(FIXTURE_DIR / "layout.ld"), "-o", str(elf), str(obj)], env=env)
    base.run_local([str(objcopy), "-O", "binary", str(elf), str(raw)], env=env)
    symbol_output = base.run_local([str(nm), "-n", str(elf)])
    symbols: dict[str, int] = {}
    for line in symbol_output.splitlines():
        fields = line.split()
        if len(fields) == 3:
            try:
                symbols[fields[2]] = int(fields[0], 16)
            except ValueError:
                continue
    image = raw.read_bytes()
    if not 0 < len(image) <= 0x1000:
        raise BuildError(f"{case}: image size {len(image)} outside 1..0x1000")
    tools = {name: {"path": str(path), "version": base.tool_version(path, "--version", env)}
             for name, path in (("clang", clang), ("ld", ld), ("objcopy", objcopy), ("nm", nm))}
    return image, {"tools": tools, "symbols": symbols}


def parse_result_block(hex_data: Any, case: str) -> dict[str, Any]:
    if not isinstance(hex_data, str) or len(hex_data) != RESULT_SIZE * 2:
        raise BuildError(f"{case}: missing or incorrectly sized result block")
    try:
        raw = bytes.fromhex(hex_data)
    except ValueError as exc:
        raise BuildError(f"{case}: malformed result block hex") from exc
    magic, version, case_id, completion, count, events, reserved = struct.unpack_from("<QIIIIII", raw)
    spec = CASES[case]
    if (magic, version, case_id, completion, reserved) != (MAGIC, 1, spec["id"], COMPLETE, 0):
        raise BuildError(f"{case}: invalid result header")
    if count > (RESULT_SIZE - 32) // 8:
        raise BuildError(f"{case}: observation count exceeds result block")
    if events != spec["events"]:
        raise BuildError(f"{case}: expected {spec['events']} recorded events, observed {events}")
    words = list(struct.unpack_from(f"<{count}Q", raw, 32)) if count else []
    return {"raw_hex": hex_data, "magic": magic, "version": version, "case_id": case_id,
            "completion": completion, "word_count": count, "event_count": events, "reserved": reserved,
            "observations": words}


def _need(words: list[int], index: int, expected: int, label: str, case: str) -> None:
    if index >= len(words) or words[index] != expected:
        actual = words[index] if index < len(words) else "missing"
        raise BuildError(f"{case}: {label} expected 0x{expected:x}, got {actual if isinstance(actual, str) else hex(actual)}")


def validate_case(case: str, result: dict[str, Any], symbols: dict[str, int] | None = None) -> dict[str, Any]:
    symbols = symbols or {}
    if result.get("exit") != "hlt" or result.get("stop_reason") != "guest_hlt":
        raise BuildError(f"{case}: expected a terminal guest HLT, got {result.get('exit')}/{result.get('stop_reason')}")
    expected_exits = [{"kind": "hlt"}, {"kind": "hlt"}] if case == "kvm_injected_interrupt" else [{"kind": "hlt"}]
    if result.get("error") or result.get("exits") != expected_exits or result.get("exits_dropped") != 0:
        raise BuildError(f"{case}: runner reported an error or an unexpected exit trace")
    if result.get("uart_hex") != "" or result.get("uart_text") != "":
        raise BuildError(f"{case}: unexpected guest UART output")
    if result.get("kvm_api") != 12 or result.get("vmx") is not True:
        raise BuildError(f"{case}: expected KVM API 12 and VMX capability")
    identity = base.hardware_identity(result)
    if not base.hardware_identity_complete(identity):
        raise BuildError(f"{case}: incomplete host or guest CPUID identity")
    final = result.get("final_state")
    if not isinstance(final, dict) or not isinstance(final.get("gpr"), dict) or not isinstance(final.get("segments"), dict) or not isinstance(final.get("tables"), dict) or not isinstance(final.get("control"), dict):
        raise BuildError(f"{case}: final CPU state is incomplete")
    for group, keys in (("gpr", ("rsp", "r15")), ("segments", ("cs", "ss")), ("tables", ("gdtr_base", "idtr_base")), ("control", ("cr0", "cr2", "cr3", "cr4", "efer"))):
        if any(key not in final[group] for key in keys):
            raise BuildError(f"{case}: final CPU state lacks {group} fields")
    for key in ("rsp", "r15"):
        if type(final["gpr"].get(key)) is not int or not 0 <= final["gpr"][key] <= 0xFFFFFFFFFFFFFFFF:
            raise BuildError(f"{case}: final CPU state has invalid {key}")
    for key in ("rip", "rflags"):
        if type(final.get(key)) is not int or not 0 <= final[key] <= 0xFFFFFFFFFFFFFFFF:
            raise BuildError(f"{case}: final CPU state has invalid {key}")
    for reg in ("cr0", "cr2", "cr3", "cr4", "efer"):
        if type(final["control"].get(reg)) is not int or not 0 <= final["control"][reg] <= 0xFFFFFFFFFFFFFFFF:
            raise BuildError(f"{case}: final CPU state has invalid {reg}")
    cs, ss = final["segments"]["cs"], final["segments"]["ss"]
    if not isinstance(cs, dict) or not isinstance(ss, dict) or type(cs.get("selector")) is not int or type(ss.get("selector")) is not int:
        raise BuildError(f"{case}: final CPU state has invalid CS or SS")
    if cs.get("selector") != 8 or cs.get("l") != 1 or cs.get("db") != 0 or cs.get("dpl") != 0:
        raise BuildError(f"{case}: terminal state is not Long64 CPL0")
    if ss.get("selector") != 0x10 or not (final["control"]["cr0"] & 0x80000000) or not (final["control"]["efer"] & 0x400):
        raise BuildError(f"{case}: terminal state is outside the expected Long64 kernel environment")
    entry = result.get("entry")
    if not isinstance(entry, dict) or any(entry.get(k) != v for k, v in base.EXPECTED_ENTRY.items()):
        raise BuildError(f"{case}: unexpected KVM entry state")
    block = parse_result_block(result.get("result_block_hex"), case)
    w = block["observations"]
    if len(w) != EXPECTED_WORDS[case]:
        raise BuildError(f"{case}: expected {EXPECTED_WORDS[case]} observations, received {len(w)}")
    # Guest-local invariants; raw words remain in evidence for independent re-analysis.
    if case == "paging_101":
        _need(w, 0, 0x11223344, "read value", case); _need(w, 5, 0x11223344, "physical sentinel", case)
        for i in (1, 2, 3, 4):
            if (w[i] & 0x21) != 0x21: raise BuildError(f"{case}: paging entry {i} must be present with accessed bit")
        if (w[4] & 0x40) or (w[4] & ~0xFFF) != 0x14000: raise BuildError(f"{case}: leaf maps wrong page or has dirty bit")
    elif case == "paging_102":
        _need(w, 0, 0x55667788, "read-after-write value", case); _need(w, 5, 0x55667788, "physical data", case)
        if any((w[i] & 0x21) != 0x21 for i in (1, 2, 3, 4)) or (w[4] & 0x60) != 0x60 or (w[4] & ~0xFFF) != 0x14000:
            raise BuildError(f"{case}: successful write A/D state invalid")
    elif case in ("paging_103", "paging_104"):
        expected_pf = {"paging_103": 0, "paging_104": 3}[case]
        _need(w, 0, expected_pf, "page-fault error code", case); _need(w, 1, 0x8000000000, "CR2", case)
        symbol = "s4_34c_fault_not_present" if case == "paging_103" else "s4_34c_fault_wp_enabled"
        _need(w, 2, symbols.get(symbol, -1), "saved fault RIP", case)
        entries = w[3:7]
        if len(entries) != 4 or any((entry & 1) == 0 for entry in entries[:3]):
            raise BuildError(f"{case}: supervisor page-walk ancestors are not present")
        if case == "paging_103" and (entries[3] & 1): raise BuildError(f"{case}: faulting leaf unexpectedly present")
        if case == "paging_104" and (entries[3] & 3) != 1: raise BuildError(f"{case}: write-protected leaf must be present and read-only")
        if case == "paging_104":
            _need(w, 7, 0x11223344, "unchanged backing value", case)
    elif case == "paging_106":
        _need(w, 0, 1, "NX capability", case)
        if not (w[1] & (1 << 20)): raise BuildError(f"{case}: CPUID does not advertise NX")
        _need(w, 2, 0x11, "page-fault error code", case)
        _need(w, 3, 0x8000000000, "CR2", case)
        _need(w, 4, 0x8000000000, "NX fetch fault RIP", case)
        if w[4] != 0x8000000000: raise BuildError(f"{case}: NX fault RIP does not equal test VA")
        if (w[8] & 3) != 3 or (w[8] & 0x000FFFFFFFFFF000) != 0x14000 or not (w[8] & (1 << 63)):
            raise BuildError(f"{case}: test leaf must be present, writable, mapped to DATA_A and NX")
        if any((w[i] & 1) == 0 for i in (5, 6, 7)):
            raise BuildError(f"{case}: NX page-walk ancestor is absent")
    elif case == "paging_105":
        _need(w, 0, 0xAABBCCDD, "WP-disabled write result", case)
        if w[5] & 0x10000: raise BuildError(f"{case}: CR0.WP was not clear during completion")
        if (w[4] & 3) != 1 or (w[4] & 0x60) != 0x60: raise BuildError(f"{case}: successful WP-disabled write leaf state invalid")
    elif case == "paging_107":
        _need(w, 0, 0x11223344, "root A value", case); _need(w, 1, 0x55667788, "root B value", case)
        if w[2] != w[6]: raise BuildError(f"{case}: active CR3 differs from root B")
        if (w[3] & ~0xFFF) != 0x14000 or (w[4] & ~0xFFF) != 0x19000 or any((w[i] & 3) != 3 for i in (3, 4)):
            raise BuildError(f"{case}: root-specific leaf mappings are incorrect")
    elif case == "paging_108":
        if len(w) != 6: raise BuildError(f"{case}: expected 6 observations")
        _need(w, 0, 0x11223344, "old mapping value", case); _need(w, 1, 0x55667788, "remapped value", case)
        _need(w, 4, 0x11223344, "old backing page", case); _need(w, 5, 0x55667788, "new backing page", case)
        if (w[2] & ~0xFFF) != 0x19000 or (w[3] & ~0xFFF) != 0x19000 or any((w[i] & 3) != 3 for i in (2, 3)):
            raise BuildError(f"{case}: remapped PTE does not point to writable DATA_B")
    elif case == "control":
        if len(w) != 13: raise BuildError(f"{case}: expected 13 observations")
        _need(w, 1, w[0] | 0x10000, "CR0.WP set", case); _need(w, 2, w[0] & ~0x10000, "CR0.WP clear", case)
        _need(w, 3, w[0], "CR0 restored", case); _need(w, 4, w[5], "CR3 readback", case)
        _need(w, 6, w[4], "CR3 restored", case)
        cpuid_edx = w[11]
        if bool(cpuid_edx & 0x2000) != bool(w[12]): raise BuildError(f"{case}: PGE support flag disagrees with CPUID.1:EDX")
        if w[12]:
            _need(w, 8, w[7] | 0x80, "CR4.PGE set", case); _need(w, 9, w[7] & ~0x80, "CR4.PGE clear", case)
        _need(w, 10, w[7], "CR4 restored", case)
    elif case == "tables_roundtrip":
        if len(w) != 25: raise BuildError(f"{case}: expected 25 observations")
        _need(w, 0, 0x37, "GDTR limit", case); _need(w, 1, 0x9000, "GDTR base", case)
        _need(w, 2, 0x40F, "IDTR limit", case); _need(w, 3, 0xC000, "IDTR base", case)
        _need(w, 4, 0x28, "TR selector", case)
        expected_gdt = [0, 0x00AF9B000000FFFF, 0x00CF93000000FFFF,
                        0x00AFFB000000FFFF, 0x00CFF3000000FFFF]
        for i, expected in enumerate(expected_gdt): _need(w, 5 + i, expected, f"GDT[{i}]", case)
        if not (w[10] & (1 << 41)) or (w[10] & ~(1 << 41)) != 0x00008900A0000067:
            raise BuildError(f"{case}: TSS descriptor is not valid and busy after LTR")
        _need(w, 11, 0, "TSS descriptor high qword", case)
        tss_rsp0 = ((w[13] & 0xFFFFFFFF) << 32) | (w[12] >> 32)
        _need([tss_rsp0], 0, 0xB000, "TSS.RSP0", case)
        if (w[24] >> 48) != 104: raise BuildError(f"{case}: TSS I/O map base is not 104")
    elif case == "int_iret_cpl0":
        if len(w) != 12: raise BuildError(f"{case}: expected 12 observations")
        _need(w, 1, w[6], "saved frame RSP", case); _need(w, 2, w[7], "saved frame SS", case)
        _need(w, 1, w[9], "RSP restored by IRETQ", case); _need(w, 2, w[10], "SS restored by IRETQ", case)
        if w[3] == 0 or (w[3] >> 48) != 0: raise BuildError(f"{case}: invalid saved frame RIP")
        _need(w, 4, 0x8, "saved frame CS", case)
        if (w[0] & 0x200) or not (w[5] & 0x200) or (w[8] & 0x200):
            raise BuildError(f"{case}: pre-INT, saved, or handler IF state is inconsistent with STI/interrupt gate")
        _need(w, 11, 1, "post-IRET marker", case)
    elif case == "cpl3_roundtrip":
        if len(w) != 7: raise BuildError(f"{case}: expected 7 observations")
        _need(w, 1, 0x1B, "user CS", case); _need(w, 4, 0x23, "user SS", case)
        _need(w, 0, symbols.get("cpl3_roundtrip_after_int", -1), "first saved user RIP", case)
        _need(w, 3, 0x16000, "user RSP", case)
        if (w[1] & 3) != 3 or (w[4] & 3) != 3: raise BuildError(f"{case}: saved user selectors lack RPL3")
        if w[0] < 0x1000 or w[5] < 0x1000: raise BuildError(f"{case}: invalid saved user RIP")
        _need(w, 6, 1, "terminal marker", case)
    elif case == "cpl3_privileged_gp":
        if len(w) != 8: raise BuildError(f"{case}: expected 8 observations")
        _need(w, 1, 0, "#GP error code", case); _need(w, 3, 0x1B, "#GP CS", case)
        _need(w, 0, 0x80000033, "CR0 before user fault", case)
        _need(w, 2, symbols.get("cpl3_privileged_user", -1), "#GP fault RIP", case)
        _need(w, 5, w[0], "CR0 in #GP handler", case)
        _need(w, 6, symbols.get("cpl3_privileged_after_int", -1), "return marker RIP", case)
        _need(w, 7, 1, "terminal marker", case)
    elif case == "kvm_injected_interrupt":
        injection = result.get("injection")
        if not isinstance(injection, dict) or injection.get("vector") != 0x41 or injection.get("delivery_observed") is not True:
            raise BuildError(f"{case}: KVM_INTERRUPT delivery was not observed")
        if injection.get("attempts") != 1 or injection.get("ioctl_result") != 0 or injection.get("event_count_before") != 0 or injection.get("event_count_after") != 1:
            raise BuildError(f"{case}: KVM_INTERRUPT request or single-delivery evidence is inconsistent")
        pre = injection.get("pre_state")
        if not isinstance(pre, dict) or any(pre.get(k) != v for k, v in {
            "cs_selector": 8, "cs_l": 1, "cs_db": 0, "cs_dpl": 0, "ss_selector": 0x10,
        }.items()):
            raise BuildError(f"{case}: pre-injection state is not Long64 CPL0")
        if not all(type(pre.get(k)) is int for k in ("rip", "rflags", "cr0", "cr3", "cr4", "efer")):
            raise BuildError(f"{case}: pre-injection control state is incomplete")
        if not (pre["cr0"] & 0x80000000) or not (pre["efer"] & 0x400):
            raise BuildError(f"{case}: pre-injection state is outside Long64 paging mode")
        _need(w, 0, 0x41, "delivered vector", case); _need(w, 5, 0x48414E444C455231, "handler marker", case)
        _need(w, 1, symbols.get("after_first_hlt", -1), "interrupt saved RIP", case)
        _need(w, 1, pre["rip"], "pre-injection RIP", case); _need(w, 3, pre["rflags"], "saved RFLAGS", case)
        _need(w, 2, 0x8, "interrupt saved CS", case)
        if not (w[3] & 0x200): raise BuildError(f"{case}: saved RFLAGS does not include STI-enabled IF")
        _need(w, 6, 0x53434B504F535431, "post-IRET resume marker", case)
        if w[4] & 0x200: raise BuildError(f"{case}: IF remained set inside interrupt gate")
    return {"status": "accepted", "result_block": block, "final_state": result["final_state"],
            "kvm_api": result.get("kvm_api"), "hardware": {k: result.get(k) for k in ("hostname", "uname", "cpu_model", "host_microcode", "vmx")},
            "guest_cpuid_model": result.get("guest_cpuid_model"), "runner": result}


def run_case(remote: base.Remote, case: str, image: bytes, workdir: Path, repeat: int,
             symbols: dict[str, int], runner_source_hash: str, runner_binary_hash: str) -> dict[str, Any]:
    remote_name = f"s4_system_{case}.bin"
    local = workdir / remote_name
    local.write_bytes(image)
    digest = sha256(image)
    remote.scp(local, remote_name)
    if remote.remote_sha(remote_name) != digest:
        return {"repeat": repeat, "image_sha256": digest, "raw_result": None,
                "error": "remote image hash mismatch before run", "validation": "rejected"}
    if remote.remote_sha("kvm_runner.c") != runner_source_hash or remote.remote_sha("kvm_runner") != runner_binary_hash:
        return {"repeat": repeat, "image_sha256": digest, "raw_result": None,
                "error": "runner source or binary hash mismatch before run", "validation": "rejected"}
    cmd = f"cd {base.shlex.quote(remote.remote_dir)} && ./kvm_runner {base.shlex.quote(remote_name)} --budget 1000 --deadline-ms 2000 --capture-final-state --result-block 0x18000:512"
    if CASES[case].get("inject") is not None:
        cmd += f" --inject-after-hlt 0x{CASES[case]['inject']:x}"
    completed = remote.ssh(cmd, timeout=60)
    integrity = {
        "image_sha256_after": remote.remote_sha(remote_name),
        "runner_source_sha256_after": remote.remote_sha("kvm_runner.c"),
        "runner_binary_sha256_after": remote.remote_sha("kvm_runner"),
    }
    attempt: dict[str, Any] = {"repeat": repeat, "image_sha256": digest, "image_size": len(image),
                               "returncode": completed.returncode, "stdout": completed.stdout,
                               "stderr": completed.stderr, "integrity": integrity}
    if integrity["image_sha256_after"] != digest:
        attempt.update(error="remote image hash changed during run", validation="rejected")
        return attempt
    if integrity["runner_source_sha256_after"] != runner_source_hash or integrity["runner_binary_sha256_after"] != runner_binary_hash:
        attempt.update(error="runner source or binary hash changed during run", validation="rejected")
        return attempt
    if completed.returncode:
        attempt.update(error=f"runner returned {completed.returncode}", validation="rejected")
        return attempt
    try:
        raw = base.parse_runner_stdout(completed.stdout)
        attempt["raw_result"] = raw
        attempt["validated"] = validate_case(case, raw, symbols)
        attempt["validation"] = "accepted"
    except (BuildError, base.BuildError) as exc:
        attempt.update(validation="rejected", validation_error=str(exc))
    return attempt


def build_evidence(remote: base.Remote, cases: list[str], workdir: Path, repeats: int = REPEATS) -> dict[str, Any]:
    if not 1 <= repeats <= 100: raise BuildError("repeats must be in 1..100")
    workdir.mkdir(parents=True, exist_ok=True)
    remote.ssh_ok(f"mkdir -p {base.shlex.quote(remote.remote_dir)}")
    remote.scp(base.RUNNER_SOURCE, "kvm_runner.c")
    source_hash = base.sha256_file(base.RUNNER_SOURCE)
    if remote.remote_sha("kvm_runner.c") != source_hash: raise BuildError("remote runner source hash mismatch")
    gcc_version = remote.compile_runner()
    binary_hash = remote.remote_sha("kvm_runner")
    runs: dict[str, Any] = {}
    tools: dict[str, Any] = {}
    for case in cases:
        image, build_details = compile_image(case, workdir)
        tools = build_details["tools"]
        attempts = [run_case(remote, case, image, workdir, n, build_details["symbols"], source_hash, binary_hash)
                    for n in range(1, repeats + 1)]
        raw_blocks = [a.get("raw_result", {}).get("result_block_hex") for a in attempts if a.get("raw_result")]
        result_stable = bool(raw_blocks) and len(raw_blocks) == len(attempts) and len(set(raw_blocks)) == 1
        identities = [base.stable_hardware_identity(base.hardware_identity(a["raw_result"]))
                      for a in attempts if isinstance(a.get("raw_result"), dict)]
        identity_stable = len(identities) == len(attempts) and bool(identities) and len({json.dumps(x, sort_keys=True) for x in identities}) == 1
        runs[case] = {"repeat_count": repeats, "stable_result_block": result_stable,
                      "stable_hardware_identity": identity_stable, "image_sha256": sha256(image),
                      "image_size": len(image), "symbols": build_details["symbols"], "attempts": attempts}
    all_attempts = [a for run in runs.values() for a in run["attempts"]]
    first_raw = next((a.get("raw_result") for a in all_attempts if isinstance(a.get("raw_result"), dict)), {})
    identity = base.hardware_identity(first_raw) if first_raw else {}
    cpuid = first_raw.get("guest_cpuid_model") if first_raw else None
    accepted = all(run["stable_result_block"] and run["stable_hardware_identity"] and
                   all(a.get("validation") == "accepted" for a in run["attempts"]) for run in runs.values())
    return {"slice": "S4-34cde", "method": "kvm_host_runner", "method_status": "accepted" if accepted else "rejected",
            "s4_34": "not_accepted", "generated_at": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "environment": "Intel VT-x via /dev/kvm on Mini; user-space KVM_INTERRUPT only for vector case; no Open Desktop CPU/MMU execution",
            "boundary": "KVM architectural fixtures; injected vector is not a physical APIC/device interrupt; S4-00/S4-01 remain prerequisites for parent acceptance",
            "remote": {"host": remote.host, "dir": remote.remote_dir}, "repeat_count_per_case": repeats,
            "hardware_identity": identity, "guest_cpuid_model": cpuid,
            "artifact_hashes": {"runner_source_local": source_hash, "runner_source_remote_final": remote.remote_sha("kvm_runner.c"),
                                "runner_binary_remote": binary_hash}, "gcc": gcc_version, "tools": tools, "runs": runs}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default=base.DEFAULT_HOST)
    parser.add_argument("--remote-dir", default=base.DEFAULT_REMOTE_DIR)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--workdir", type=Path, default=DEFAULT_WORKDIR)
    parser.add_argument("--repeats", type=int, default=REPEATS)
    parser.add_argument("--cases", nargs="+", choices=list(CASES), default=list(CASES))
    args = parser.parse_args(argv)
    try:
        evidence = build_evidence(base.Remote(args.host, args.remote_dir), args.cases, args.workdir, args.repeats)
    except (BuildError, base.BuildError, OSError, base.subprocess.TimeoutExpired) as exc:
        print(f"error: {exc}", file=sys.stderr); return 1
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"{evidence['method_status']} ({len(evidence['runs'])} cases x {args.repeats} repeats): {args.output}")
    for case, run in evidence["runs"].items():
        rejected = [a.get("validation_error", a.get("error")) for a in run["attempts"] if a.get("validation") != "accepted"]
        if rejected: print(f"  {case}: {rejected}")
    return 0 if evidence["method_status"] == "accepted" else 1


if __name__ == "__main__":
    raise SystemExit(main())
