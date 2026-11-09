#!/usr/bin/env python3
"""Build and validate the freestanding Stage4InterruptTimerProbeV1 kernel image (S4-27i2).

The image is a flat identity-mapped Long64/CPL0 guest for the owner-driven timer
pump: GDT, an IDT with one gate (vector 0x61), one code page and a counter dword
at 0x5000 (written by the handler). The loader installs IDTR, so the image has no
IDTR page. The raw image is loaded at GPA 0x1000 and is exactly 0x4000 bytes. The
manifest records the expected evidence derived from the Intel SDM and the APIC
timer formula (never from the interpreter's output):

* SDM Vol. 3A 10.5.4 (APIC Timer): the current count decrements at the timer
  input frequency divided by the Divide Configuration divisor; the divisor is
  selected by bits 3, 1 and 0 of the register (0b0000 -> 2, 0b0001 -> 4,
  0b0010 -> 8, 0b0011 -> 16, 0b1000 -> 32, 0b1001 -> 64, 0b1010 -> 128,
  0b1011 -> 1). In periodic mode (LVT Timer bit 17) the initial count is
  reloaded when the count reaches zero, so a vector is raised every
  initial_count * divisor timer ticks. With Initial Count 625, divide by 16 and a
  100 MHz timer input the period is P = 625 * 16 / 100e6 s = 100 us = 100000 ns.
  The guest arms the timer at virtual time 0 (instructions take 0 ns in S4-27i2),
  so the k-th expiry is at k * P and after N wake-ups the virtual time is N * P.
* SDM Vol. 3A 6.14.2/6.14.3: a 64-bit interrupt gate pushes SS, RSP, RFLAGS, CS,
  RIP (five qwords, no error code for an external interrupt) after aligning RSP
  down to 16 bytes, and clears IF. The stack top 0x6ff0 is 16-byte aligned, so
  the new RSP is 0x6ff0 - 40 = 0x6fc8 and the pushed RFLAGS is 0x202. An interrupt
  that wakes HLT pushes the RIP of the instruction after HLT.
* SDM Vol. 3A 10.8.3.1: EOI clears the in-service bit.
The `unarmed` variant never programs the timer (the halted guest has no deadline).
"""

from __future__ import annotations

import argparse
import json
import os
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_stage4_flat_probe import (  # noqa: E402
    BuildError,
    elf_symbol,
    output_directory,
    parse_elf_load_segments,
    resolve_executable,
    run,
    rust_toolchain,
    sha256,
)

IMAGE_BASE = 0x1000
IMAGE_SIZE = 0x4000
GDT_GPA = 0x2000
IDT_GPA = 0x3000
CODE_GPA = 0x4000
COUNTER_GPA = 0x5000
STACK_TOP = 0x6FF0
VECTOR = 0x61
GDT = struct.pack("<QQQ", 0, 0x00AF9B000000FFFF, 0x00CF93000000FFFF)
SCENARIOS = {"periodic": 0, "unarmed": 1}
# Timer configuration written by the guest and the host timer input frequency.
TIMER_HZ = 100_000_000
DIVIDE_CONFIG = 0x3
INITIAL_COUNT = 625
LVT_TIMER = 0x20061  # periodic (bit 17), unmasked, vector 0x61
DEFAULT_WAKEUPS = 5
MAX_WAKEUPS = 127  # CMP EAX,imm8 is sign-extended
# Instruction encodings (Intel SDM Vol. 2): MOV r32,imm32 = B8+r; MOV r/m32,r32 = 89 /r
# (ModRM 0x87 = [RDI+disp32],EAX; 0x06 = [RSI],EAX); MOV EAX,[RSI] 8B 06; ADD r/m32,imm8
# 83 /0 (C0 = EAX); CMP r/m32,imm8 83 /7 (F8 = EAX); JNE rel8 75; STI FB; HLT F4;
# JMP rel8 EB; IRETQ 48 CF.
MOV_EDI = bytes.fromhex("bf0000e0fe")  # MOV EDI,0xFEE00000
MOV_ESI = bytes.fromhex("be00500000")  # MOV ESI,0x5000


def mov_eax(value: int) -> bytes:
    return b"\xb8" + struct.pack("<I", value)


def store_eax(disp: int) -> bytes:
    return bytes.fromhex("8987") + struct.pack("<I", disp)  # MOV [RDI+disp],EAX


def write_reg(disp: int, value: int) -> bytes:
    return mov_eax(value) + store_eax(disp)


def divisor(config: int) -> int:
    """Divisor selected by the Divide Configuration register (SDM Vol. 3A 10.5.4)."""
    index = (config & 3) | ((config >> 1) & 4)
    return 1 if index == 7 else 2 << index


def period_ns() -> int:
    ticks = INITIAL_COUNT * divisor(DIVIDE_CONFIG)
    nanoseconds, remainder = divmod(ticks * 1_000_000_000, TIMER_HZ)
    if remainder:
        raise BuildError("the timer period is not a whole number of nanoseconds")
    return nanoseconds


def prologue(scenario: str) -> bytes:
    code = MOV_EDI + write_reg(0xF0, 0x1FF)  # SVR = 0x1FF: software enable
    if scenario == "periodic":
        code += write_reg(0x3E0, DIVIDE_CONFIG)
        code += write_reg(0x320, LVT_TIMER)
        code += write_reg(0x380, INITIAL_COUNT)  # arms the timer
    return code


def entry_code(scenario: str) -> bytes:
    # STI; loop: HLT; JMP loop (rel8 = -3).
    return prologue(scenario) + bytes.fromhex("fbf4ebfd")


def handler_code(wakeups: int) -> bytes:
    cancel = mov_eax(0) + store_eax(0x380)  # Initial Count = 0 cancels the timer
    return (
        MOV_EDI
        + MOV_ESI
        + bytes.fromhex("8b06")  # MOV EAX,[RSI]
        + bytes.fromhex("83c001")  # ADD EAX,1
        + bytes.fromhex("8906")  # MOV [RSI],EAX
        + bytes.fromhex("83f8") + bytes([wakeups])  # CMP EAX,wakeups
        + bytes([0x75, len(cancel)])  # JNE ack
        + cancel
        + write_reg(0xB0, 0)  # EOI
        + bytes.fromhex("48cf")  # IRETQ
    )


def expected_evidence(scenario: str, wakeups: int) -> dict:
    """Expected evidence; addresses follow from the encodings, the rest is SDM arithmetic."""
    entry = entry_code(scenario)
    hlt = CODE_GPA + len(prologue(scenario)) + 1  # after the one-byte STI
    handler = CODE_GPA + len(entry)
    period = period_ns()
    frame_rsp = STACK_TOP - 5 * 8
    ev = lambda name, vector=None: {"event": name} if vector is None else {"event": name, "vector": vector}  # noqa: E731
    events = [ev("sti"), ev("hlt")]
    final = {"exit": "idle_halt", "rip": f"0x{hlt + 1:x}", "rsp": f"0x{STACK_TOP:x}", "rflags": "0x202"}
    if scenario == "periodic":
        for _ in range(wakeups):
            events += [ev("irr", VECTOR), {"event": "entry", "vector": VECTOR, "source": "halt_wake"},
                       ev("eoi", VECTOR), ev("iretq"), ev("hlt")]
        count = wakeups
    else:
        count = 0
    wake_times = [k * period for k in range(1, count + 1)]
    return {
        "timer": {
            "timer_hz": TIMER_HZ,
            "divide_config": f"0x{DIVIDE_CONFIG:x}",
            "divisor": divisor(DIVIDE_CONFIG),
            "initial_count": INITIAL_COUNT,
            "lvt_timer": f"0x{LVT_TIMER:x}",
            "vector": f"0x{VECTOR:x}",
            "period_ns": period,
            "formula": "period_ns = initial_count * divisor * 1e9 / timer_hz",
        },
        "wakeups": count,
        "counter": count,
        "counter_gpa": f"0x{COUNTER_GPA:x}",
        "wake_times_ns": wake_times,
        "time_ns": count * period,
        "hlt": f"0x{hlt:x}",
        "handler": f"0x{handler:x}",
        # SDM 64-bit interrupt frame, low to high address; RIP is the RIP after HLT.
        "frame": {
            "address": f"0x{frame_rsp:x}",
            "rip": f"0x{hlt + 1:x}",
            "cs": "0x8",
            "rflags": "0x202",
            "rsp": f"0x{STACK_TOP:x}",
            "ss": "0x10",
        },
        "handler_rflags": "0x2",
        "events": events,
        "final": {**final, "vector": {f"0x{VECTOR:x}": [False, False]}, "timer_deadline_ns": None},
    }


def validate_image(elf: bytes, raw: bytes, args: argparse.Namespace) -> dict:
    entry, segments = parse_elf_load_segments(elf)
    code_start, _ = elf_symbol(elf, "_start")
    handler_gpa, _ = elf_symbol(elf, "stage4_timer_handler")
    code_end, _ = elf_symbol(elf, "stage4_code_end")
    if entry != CODE_GPA or code_start != CODE_GPA:
        raise BuildError(f"entry is 0x{entry:x}, expected 0x{CODE_GPA:x}")
    entry_bytes = entry_code(args.scenario)
    handler_bytes = handler_code(args.wakeups)
    if handler_gpa != CODE_GPA + len(entry_bytes) or code_end != handler_gpa + len(handler_bytes):
        raise BuildError(
            f"unexpected code layout: handler=0x{handler_gpa:x} end=0x{code_end:x} "
            f"entry_len=0x{len(entry_bytes):x}"
        )
    code_len = len(entry_bytes) + len(handler_bytes)
    expected = [
        (GDT_GPA, GDT_GPA, len(GDT), 6),
        (IDT_GPA, IDT_GPA, 0x1000, 6),
        # The linker rounds the code output section up to a multiple of four bytes.
        (CODE_GPA, CODE_GPA, (code_len + 3) & ~3, 5),
    ]
    if len(segments) != len(expected):
        raise BuildError(f"expected {len(expected)} PT_LOAD segments, found {len(segments)}")
    for segment, (vaddr, paddr, size, flags) in zip(segments, expected, strict=True):
        actual = (segment["vaddr"], segment["paddr"], segment["filesz"], segment["flags"])
        if actual != (vaddr, paddr, size, flags) or segment["memsz"] != size:
            raise BuildError(f"unexpected PT_LOAD {actual}, expected {(vaddr, paddr, size, flags)}")
    if len(raw) != IMAGE_SIZE:
        raise BuildError(f"raw image is 0x{len(raw):x} bytes, expected 0x{IMAGE_SIZE:x}")
    for segment in segments:
        start = segment["paddr"] - IMAGE_BASE
        if raw[start : start + segment["filesz"]] != elf[segment["offset"] : segment["offset"] + segment["filesz"]]:
            raise BuildError(f"raw bytes differ from ELF at GPA 0x{segment['paddr']:x}")

    def at(gpa: int, length: int) -> bytes:
        return raw[gpa - IMAGE_BASE : gpa - IMAGE_BASE + length]

    if at(GDT_GPA, len(GDT)) != GDT:
        raise BuildError("GDT descriptors do not match the fixed profile")
    idt = at(IDT_GPA, 0x1000)
    for slot in range(256):
        gate = idt[slot * 16 : slot * 16 + 16]
        if slot == VECTOR:
            fields = struct.unpack("<HHBBHII", gate)
            if fields != (handler_gpa, 8, 0, 0x8E, 0, 0, 0):
                raise BuildError(f"IDT gate for vector 0x{slot:x} is wrong: {gate.hex()}")
        elif gate != bytes(16):
            raise BuildError(f"IDT must contain no gate other than 0x{VECTOR:x}")
    if at(CODE_GPA, len(entry_bytes)) != entry_bytes:
        raise BuildError("entry code does not match the selected scenario")
    if at(handler_gpa, len(handler_bytes)) != handler_bytes:
        raise BuildError("handler code does not match the selected variant")
    covered = [(GDT_GPA, len(GDT)), (IDT_GPA, 0x1000), (CODE_GPA, code_len)]
    for gpa in range(IMAGE_BASE, IMAGE_BASE + IMAGE_SIZE):
        if raw[gpa - IMAGE_BASE] and not any(start <= gpa < start + size for start, size in covered):
            raise BuildError(f"non-zero byte outside the fixed layout at GPA 0x{gpa:x}")
    return {
        "entry_gpa": f"0x{entry:x}",
        "handler_gpa": f"0x{handler_gpa:x}",
        "code_end_gpa": f"0x{code_end:x}",
        "gdt_gpa": f"0x{GDT_GPA:x}",
        "idt_gpa": f"0x{IDT_GPA:x}",
        "counter_gpa": f"0x{COUNTER_GPA:x}",
        "stack_top": f"0x{STACK_TOP:x}",
        "vector": f"0x{VECTOR:x}",
        "load_gpa": f"0x{IMAGE_BASE:x}",
        "image_size": f"0x{IMAGE_SIZE:x}",
        "segments": [
            {"paddr": f"0x{p:x}", "filesz": f"0x{s:x}", "flags": f"0x{f:x}"} for _, p, s, f in expected
        ],
    }


def build(args: argparse.Namespace) -> int:
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/interrupt_timer"
    if args.scenario not in SCENARIOS:
        raise BuildError(f"--scenario must be one of {sorted(SCENARIOS)}")
    if not 1 <= args.wakeups <= MAX_WAKEUPS:
        raise BuildError(f"--wakeups must be between 1 and {MAX_WAKEUPS}")
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    defines = [
        f"-DPROBE_SCENARIO={SCENARIOS[args.scenario]}",
        f"-DPROBE_WAKEUPS={args.wakeups}",
    ]
    commands = [
        [str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp", "-ffreestanding",
         "-fno-pic", "-fno-pie", "-mno-red-zone", *defines, "-c", str(fixture / "entry.S"),
         "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T", str(fixture / "layout.ld"),
         "-o", str(out / "probe.elf"), str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00", str(out / "probe.elf"),
         str(out / "probe.unpadded.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "probe.elf").read_bytes()
    # objcopy starts at the lowest load address (the GDT at 0x2000).
    body = (out / "probe.unpadded.bin").read_bytes()
    raw = bytes(GDT_GPA - IMAGE_BASE) + body
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"image exceeds its fixed size: 0x{len(raw):x}")
    raw = raw.ljust(IMAGE_SIZE, b"\0")
    layout = validate_image(elf, raw, args)
    (out / "probe.bin").write_bytes(raw)
    (out / "probe.unpadded.bin").unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    variant = {"scenario": args.scenario, "wakeups": args.wakeups}
    manifest = {
        "schema": "stage4-interrupt-timer-probe-build-v1",
        "profile": "Stage4InterruptTimerProbeV1",
        "variant": variant,
        "accepted_profile": True,
        "base_revision": revision,
        "source": {name: sha256((fixture / name).read_bytes()) for name in ("entry.S", "layout.ld")},
        "target": "x86_64-unknown-none-elf",
        "toolchain": {
            "clang": clang_version,
            "ld.lld": linker_version,
            "rust-objcopy": objcopy_version.splitlines()[0] if objcopy_version else "",
            "rustc": rustc_version.splitlines()[0],
            "host": host,
        },
        "commands": commands,
        "layout": layout,
        "expected": expected_evidence(args.scenario, args.wakeups),
        "artifacts": {
            "probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"output: {out}")
    print(f"raw sha256: {manifest['artifacts']['probe.bin']['sha256']}; variant: {variant}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), default="periodic")
    parser.add_argument("--wakeups", type=int, default=DEFAULT_WAKEUPS)
    args = parser.parse_args()
    try:
        return build(args)
    except (BuildError, OSError) as error:
        print(f"build_stage4_interrupt_timer_probe: error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
