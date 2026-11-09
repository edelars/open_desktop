#!/usr/bin/env python3
"""Build and validate the freestanding Stage4InterruptPumpProbeV1 kernel image (S4-27i1).

The image is a flat identity-mapped Long64/CPL0 guest for the event pump: GDT, an
IDT with two gates (vectors 0x41 and 0x51) and one code page. The loader
installs IDTR, so the image has no IDTR page. The raw image is loaded at GPA
0x1000 and is exactly 0x4000 bytes. One scenario is selected at build time; the
manifest records the expected evidence derived from the Intel SDM (never from
the interpreter's output):

* SDM Vol. 3A 6.14.2/6.14.3: a 64-bit interrupt gate pushes SS, RSP, RFLAGS, CS,
  RIP (five qwords, no error code for an external interrupt) after aligning RSP
  down to 16 bytes, and clears IF. The stack top 0x6ff0 is 16-byte aligned, so
  the new RSP is 0x6ff0 - 40 = 0x6fc8 and the pushed RFLAGS is 0x202.
* SDM Vol. 2B STI/HLT: STI delays interrupt acceptance by one instruction, so
  after STI the next boundary accepts nothing and HLT retires first; an
  interrupt that wakes HLT pushes the RIP of the instruction after HLT. With
  IF=1 and no shadow, an interrupt pending at a boundary before HLT is accepted
  there and the pushed RIP is the address of HLT. With IF=0 only NMI/SMI/INIT/
  RESET wake HLT, so a pending maskable vector does not.
* SDM Vol. 3A 10.8.3.1: a request is accepted only if its priority class is
  above the processor priority (highest in-service class when TPR is 0); EOI
  clears the in-service bit.
Variants exist only to prove that broken guests fail the acceptance check.
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
STACK_TOP = 0x6FF0
VECTOR_A = 0x51
VECTOR_B = 0x41
GDT = struct.pack("<QQQ", 0, 0x00AF9B000000FFFF, 0x00CF93000000FFFF)
SCENARIOS = {"sti_hlt": 0, "if1_hlt": 1, "cli_hlt": 2, "rep": 3}
# Instruction encodings (Intel SDM Vol. 2): MOV r32,imm32 = B8+r; MOV r/m32,r32
# = 89 /r with ModRM 0x87 for [RDI+disp32],EAX; STI FB; CLI FA; HLT F4; NOP 90;
# REP STOSB F3 AA; JMP rel8 EB; IRETQ 48 CF.
MOV_EDI = bytes.fromhex("bf0000e0fe")  # MOV EDI,0xFEE00000
STORE_EAX = lambda disp: bytes.fromhex("8987") + struct.pack("<I", disp)  # MOV [RDI+disp],EAX
SPIN = bytes.fromhex("ebfe")


def prologue() -> bytes:
    return MOV_EDI + bytes.fromhex("b8ff010000") + STORE_EAX(0xF0)  # SVR = 0x1FF


def scenario_code(scenario: str) -> bytes:
    body = {
        "sti_hlt": bytes.fromhex("fbf4f4"),
        "if1_hlt": bytes.fromhex("fb90f4"),
        "cli_hlt": bytes.fromhex("faf4"),
        "rep": bytes.fromhex("fb90") + bytes.fromhex("b9d0070000") + bytes.fromhex("bf00500000")
        + bytes.fromhex("f3aa") + bytes.fromhex("f4"),
    }[scenario]
    return prologue() + body + SPIN


def eoi(omit: bool) -> bytes:
    return b"" if omit else bytes.fromhex("b800000000") + STORE_EAX(0xB0)


def handler_a(omit_eoi: bool) -> bytes:
    # Self-IPI (ICR = fixed, assert, shorthand self, vector 0x41), EOI, IRETQ.
    return MOV_EDI + bytes.fromhex("b841400400") + STORE_EAX(0x300) + eoi(omit_eoi) + bytes.fromhex("48cf")


def handler_b(omit_eoi: bool) -> bytes:
    return MOV_EDI + eoi(omit_eoi) + bytes.fromhex("48cf")


def expected_evidence(scenario: str, vector: int, omit_eoi: bool) -> dict:
    """Expected evidence for the scenario with the caller requesting `vector`.

    Addresses follow from the encodings above; everything else is SDM arithmetic.
    """
    entry = scenario_code(scenario)
    handlers = {VECTOR_A: CODE_GPA + len(entry), VECTOR_B: CODE_GPA + len(entry) + len(handler_a(omit_eoi))}
    prologue_len = len(prologue())
    hlt1 = CODE_GPA + prologue_len + {"sti_hlt": 1, "if1_hlt": 2, "cli_hlt": 1, "rep": 1 + 1 + 5 + 5 + 2}[scenario]
    spin = CODE_GPA + len(entry) - len(SPIN)
    frame_rsp = STACK_TOP - 5 * 8
    frame = lambda rip: {  # noqa: E731 - SDM 64-bit interrupt stack frame, low to high address
        "address": f"0x{frame_rsp:x}",
        "rip": f"0x{rip:x}",
        "cs": "0x8",
        "rflags": "0x202",
        "rsp": f"0x{STACK_TOP:x}",
        "ss": "0x10",
    }
    sti_events = [{"event": "sti"}]
    ev = lambda name, v=None: {"event": name} if v is None else {"event": name, "vector": v}  # noqa: E731
    request_after = 3
    final = {"exit": "idle_halt", "rip": f"0x{hlt1 + 1:x}", "rsp": f"0x{STACK_TOP:x}", "rflags": "0x202"}
    irr_isr = {VECTOR_B: [False, False], VECTOR_A: [False, False]}
    if scenario == "sti_hlt":
        # Execution resumes at the second HLT, which then halts for good.
        final["rip"] = f"0x{hlt1 + 2:x}"
    frames = []
    if scenario == "cli_hlt":
        if vector != VECTOR_B:
            raise BuildError("cli_hlt requests vector 0x41")
        events = [ev("irr", VECTOR_B), ev("cli"), ev("hlt")]
        final["rflags"] = "0x2"
        irr_isr[VECTOR_B] = [True, False]
        frames = []
    elif scenario == "sti_hlt" and vector == VECTOR_A:
        events = [ev("irr", VECTOR_A), *sti_events, ev("hlt"), ev("entry", VECTOR_A), ev("irr", VECTOR_B)]
        if omit_eoi:
            irr_isr[VECTOR_B] = [True, False]
            irr_isr[VECTOR_A] = [False, True]
            events += [ev("iretq"), ev("hlt")]
        else:
            events += [ev("eoi", VECTOR_A), ev("iretq"), ev("entry", VECTOR_B), ev("eoi", VECTOR_B), ev("iretq"), ev("hlt")]
        # A wakes from HLT (frame RIP after HLT); B is accepted at the boundary
        # before the second HLT after A's EOI and IRETQ (frame RIP = that HLT).
        frames = [frame(hlt1 + 1)] if omit_eoi else [frame(hlt1 + 1), frame(hlt1 + 1)]
    elif scenario == "sti_hlt":
        events = [ev("irr", VECTOR_B), *sti_events, ev("hlt"), ev("entry", VECTOR_B)]
        events += [ev("iretq"), ev("hlt")] if omit_eoi else [ev("eoi", VECTOR_B), ev("iretq"), ev("hlt")]
        irr_isr[VECTOR_B] = [False, omit_eoi]
        frames = [frame(hlt1 + 1)]
    elif scenario == "if1_hlt":
        events = [ev("irr", VECTOR_B), *sti_events, ev("entry", VECTOR_B)]
        events += [ev("iretq"), ev("hlt")] if omit_eoi else [ev("eoi", VECTOR_B), ev("iretq"), ev("hlt")]
        irr_isr[VECTOR_B] = [False, omit_eoi]
        frames = [frame(hlt1)]
        final["rip"] = f"0x{hlt1 + 1:x}"
    else:  # rep
        request_after = 8
        events = [*sti_events, ev("irr", VECTOR_B), ev("entry", VECTOR_B)]
        events += [ev("iretq"), ev("hlt")] if omit_eoi else [ev("eoi", VECTOR_B), ev("iretq"), ev("hlt")]
        irr_isr[VECTOR_B] = [False, omit_eoi]
        frames = [frame(hlt1)]
    if scenario == "sti_hlt" and vector == VECTOR_A and not omit_eoi:
        irr_isr = {VECTOR_B: [False, False], VECTOR_A: [False, False]}
    final["vectors"] = {f"0x{v:x}": state for v, state in irr_isr.items()}
    return {
        "request": {"vector": f"0x{vector:x}", "after_quanta": request_after},
        "handler_a": f"0x{handlers[VECTOR_A]:x}",
        "handler_b": f"0x{handlers[VECTOR_B]:x}",
        "hlt": f"0x{hlt1:x}",
        "spin": f"0x{spin:x}",
        "frames": frames,
        "handler_rflags": "0x2",
        "events": events,
        "final": final,
    }


def validate_image(elf: bytes, raw: bytes, args: argparse.Namespace) -> dict:
    entry, segments = parse_elf_load_segments(elf)
    code_start, _ = elf_symbol(elf, "_start")
    a_gpa, _ = elf_symbol(elf, "stage4_handler_a")
    b_gpa, _ = elf_symbol(elf, "stage4_handler_b")
    code_end, _ = elf_symbol(elf, "stage4_code_end")
    if entry != CODE_GPA or code_start != CODE_GPA:
        raise BuildError(f"entry is 0x{entry:x}, expected 0x{CODE_GPA:x}")
    entry_code = scenario_code(args.scenario)
    a_code = handler_a(args.omit_eoi)
    b_code = handler_b(args.omit_eoi)
    if (
        a_gpa != CODE_GPA + len(entry_code)
        or b_gpa != a_gpa + len(a_code)
        or code_end != b_gpa + len(b_code)
    ):
        raise BuildError(
            f"unexpected code layout: a=0x{a_gpa:x} b=0x{b_gpa:x} end=0x{code_end:x} "
            f"entry_len=0x{len(entry_code):x}"
        )
    code_len = len(entry_code) + len(a_code) + len(b_code)
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
    gates = {VECTOR_B: b_gpa, VECTOR_A: a_gpa}
    for slot in range(256):
        gate = idt[slot * 16 : slot * 16 + 16]
        if slot in gates:
            fields = struct.unpack("<HHBBHII", gate)
            if fields != (gates[slot], 8, 0, 0x8E, 0, 0, 0):
                raise BuildError(f"IDT gate for vector 0x{slot:x} is wrong: {gate.hex()}")
        elif gate != bytes(16):
            raise BuildError(f"IDT must contain no gate other than 0x{VECTOR_B:x} and 0x{VECTOR_A:x}")
    if at(CODE_GPA, len(entry_code)) != entry_code:
        raise BuildError("entry code does not match the selected scenario")
    if at(a_gpa, len(a_code)) != a_code:
        raise BuildError("handler A code does not match the selected variant")
    if at(b_gpa, len(b_code)) != b_code:
        raise BuildError("handler B code does not match the selected variant")
    covered = [(GDT_GPA, len(GDT)), (IDT_GPA, 0x1000), (CODE_GPA, code_len)]
    for gpa in range(IMAGE_BASE, IMAGE_BASE + IMAGE_SIZE):
        if raw[gpa - IMAGE_BASE] and not any(start <= gpa < start + size for start, size in covered):
            raise BuildError(f"non-zero byte outside the fixed layout at GPA 0x{gpa:x}")
    return {
        "entry_gpa": f"0x{entry:x}",
        "handler_a_gpa": f"0x{a_gpa:x}",
        "handler_b_gpa": f"0x{b_gpa:x}",
        "code_end_gpa": f"0x{code_end:x}",
        "gdt_gpa": f"0x{GDT_GPA:x}",
        "idt_gpa": f"0x{IDT_GPA:x}",
        "stack_top": f"0x{STACK_TOP:x}",
        "vector_a": f"0x{VECTOR_A:x}",
        "vector_b": f"0x{VECTOR_B:x}",
        "load_gpa": f"0x{IMAGE_BASE:x}",
        "image_size": f"0x{IMAGE_SIZE:x}",
        "segments": [
            {"paddr": f"0x{p:x}", "filesz": f"0x{s:x}", "flags": f"0x{f:x}"} for _, p, s, f in expected
        ],
    }


def build(args: argparse.Namespace) -> int:
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/interrupt_pump"
    if args.scenario not in SCENARIOS:
        raise BuildError(f"--scenario must be one of {sorted(SCENARIOS)}")
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    defines = [
        f"-DPROBE_SCENARIO={SCENARIOS[args.scenario]}",
        f"-DPROBE_OMIT_EOI={int(args.omit_eoi)}",
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
    variant = {"scenario": args.scenario, "omit_eoi": args.omit_eoi}
    # The nested case (self-IPI B during handler A) reuses the sti_hlt image and
    # is selected by the caller's request vector, so both are recorded.
    evidence = {
        "request_b": expected_evidence(args.scenario, VECTOR_B, args.omit_eoi),
    }
    if args.scenario == "sti_hlt":
        evidence["request_a_nested"] = expected_evidence(args.scenario, VECTOR_A, args.omit_eoi)
    manifest = {
        "schema": "stage4-interrupt-pump-probe-build-v1",
        "profile": "Stage4InterruptPumpProbeV1",
        "variant": variant,
        "accepted_profile": args.omit_eoi is False,
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
        "expected": evidence,
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
    parser.add_argument("--scenario", choices=sorted(SCENARIOS), default="sti_hlt")
    parser.add_argument("--omit-eoi", action="store_true")
    args = parser.parse_args()
    try:
        return build(args)
    except (BuildError, OSError) as error:
        print(f"build_stage4_interrupt_pump_probe: error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
