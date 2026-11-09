#!/usr/bin/env python3
"""Build and validate the freestanding Stage4InterruptQueueProbeV1 kernel image (S4-27i3).

The image is a flat identity-mapped Long64/CPL0 guest for the IRQ event queue of the
pump: GDT, an IDT with one gate (vector 0x62), one code page and a counter dword at
0x5000 (written by the handler). The loader installs IDTR, so the image has no IDTR
page. The raw image is loaded at GPA 0x1000 and is exactly 0x4000 bytes. The guest
programs no device: it software-enables the Local APIC, runs `STI; loop: HLT; JMP
loop` and counts interrupts in the handler. Every interrupt comes from events the
caller schedules in the pump's queue. The manifest records the expected evidence of
the fixed caller schedule (below), derived from the rules of the queue and the Intel
SDM (never from the interpreter's output):

* Queue rules: an event whose deadline is at or before the virtual time is due at the
  next boundary; a later event is delivered when the halted CPU's clock reaches its
  deadline (idle advance), so the time after that wake is exactly the deadline
  (instructions take 0 ns). A due event is removed only after `request_fixed_edge`
  succeeded (a failed request keeps it). Two due events with one vector make one IRR
  edge (the second is merged), hence one interrupt entry.
* Schedule (all events use vector 0x62; keys are 1..4):
  phase 1 - the host disables the APIC and schedules E1 (due 0 ns) and F (due
  250000 ns); the request fails, the run stops with a retained queue of two events
  and the guest has not moved;
  phase 2 - the host enables the APIC; the guest enables the vector, STI, HLT; E1
  wakes it at 0 ns and F at 250000 ns;
  phase 3 - the host schedules P1 and P2 (both due 250000 ns, the current time): one
  merged IRR edge, one wake. Totals: 3 wake-ups, counter 3, time 250000 ns, 1 merged
  event. The injection that removes an event before its request loses E1: 2 wake-ups,
  counter 2.
* SDM Vol. 3A 6.14.2/6.14.3: a 64-bit interrupt gate pushes SS, RSP, RFLAGS, CS, RIP
  (five qwords, no error code for an external interrupt) after aligning RSP down to 16
  bytes, and clears IF. The stack top 0x6ff0 is 16-byte aligned, so the new RSP is
  0x6ff0 - 40 = 0x6fc8 and the pushed RFLAGS is 0x202. An interrupt that wakes HLT
  pushes the RIP of the instruction after HLT.
* SDM Vol. 3A 10.8.3.1: EOI clears the in-service bit.
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
VECTOR = 0x62
GDT = struct.pack("<QQQ", 0, 0x00AF9B000000FFFF, 0x00CF93000000FFFF)
# Caller schedule of the probe (virtual ns). The runner and the machine tests use it too.
FUTURE_NS = 250_000
PHASE1 = [(1, 0), (2, FUTURE_NS)]  # (key, due_ns): E1 due now, F in the future
PHASE3 = [(3, FUTURE_NS), (4, FUTURE_NS)]  # P1, P2: both due at the current time
# Instruction encodings (Intel SDM Vol. 2): MOV r32,imm32 = B8+r; MOV r/m32,r32 = 89 /r
# (ModRM 0x87 = [RDI+disp32],EAX; 0x06 = [RSI],EAX); MOV EAX,[RSI] 8B 06; ADD r/m32,imm8
# 83 /0 (C0 = EAX); STI FB; HLT F4; JMP rel8 EB; IRETQ 48 CF.
MOV_EDI = bytes.fromhex("bf0000e0fe")  # MOV EDI,0xFEE00000
MOV_ESI = bytes.fromhex("be00500000")  # MOV ESI,0x5000


def mov_eax(value: int) -> bytes:
    return b"\xb8" + struct.pack("<I", value)


def store_eax(disp: int) -> bytes:
    return bytes.fromhex("8987") + struct.pack("<I", disp)  # MOV [RDI+disp],EAX


def write_reg(disp: int, value: int) -> bytes:
    return mov_eax(value) + store_eax(disp)


def prologue() -> bytes:
    return MOV_EDI + write_reg(0xF0, 0x1FF)  # SVR = 0x1FF: software enable


def entry_code() -> bytes:
    # STI; loop: HLT; JMP loop (rel8 = -3).
    return prologue() + bytes.fromhex("fbf4ebfd")


def handler_code() -> bytes:
    return (
        MOV_EDI
        + MOV_ESI
        + bytes.fromhex("8b06")  # MOV EAX,[RSI]
        + bytes.fromhex("83c001")  # ADD EAX,1
        + bytes.fromhex("8906")  # MOV [RSI],EAX
        + write_reg(0xB0, 0)  # EOI
        + bytes.fromhex("48cf")  # IRETQ
    )


def expected_evidence() -> dict:
    """Expected evidence; addresses follow from the encodings, the rest is queue-rule arithmetic."""
    hlt = CODE_GPA + len(prologue()) + 1  # after the one-byte STI
    handler = CODE_GPA + len(entry_code())
    frame_rsp = STACK_TOP - 5 * 8
    ev = lambda name, vector=None: {"event": name} if vector is None else {"event": name, "vector": vector}  # noqa: E731
    entry = {"event": "entry", "vector": VECTOR, "source": "halt_wake"}
    # Phase 1 emits nothing (the request fails before any state change). Phase 2: the
    # retried request of E1 puts the vector in the IRR at the first boundary; STI, HLT,
    # the wake (E1), handler EOI, IRETQ, HLT; the idle advance reaches F, which wakes
    # the CPU again. Phase 3: P1 raises the IRR edge, P2 merges into it, one wake.
    events = [
        ev("irr", VECTOR), ev("sti"), ev("hlt"), entry, ev("eoi", VECTOR), ev("iretq"), ev("hlt"),
        ev("irr", VECTOR), entry, ev("eoi", VECTOR), ev("iretq"), ev("hlt"),
        ev("irr", VECTOR), ev("coalesced", VECTOR), entry, ev("eoi", VECTOR), ev("iretq"), ev("hlt"),
    ]
    wake_times = [0, FUTURE_NS, FUTURE_NS]
    return {
        "schedule": {
            "vector": f"0x{VECTOR:x}",
            "phase1_apic_disabled": [{"key": k, "due_ns": d} for k, d in PHASE1],
            "phase3_after_idle": [{"key": k, "due_ns": d} for k, d in PHASE3],
        },
        "phase1": {
            "exit": "rejected",
            "error": "QueueRequest",
            "retired": 0,
            "queue_len": len(PHASE1),
            "rip": f"0x{CODE_GPA:x}",
        },
        "wakeups": len(wake_times),
        "counter": len(wake_times),
        "counter_gpa": f"0x{COUNTER_GPA:x}",
        "wake_times_ns": wake_times,
        "time_ns": FUTURE_NS,
        "coalesced": 1,
        "queue_len": 0,
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
        "final": {
            "exit": "idle_halt",
            "rip": f"0x{hlt + 1:x}",
            "rsp": f"0x{STACK_TOP:x}",
            "rflags": "0x202",
            "vector": {f"0x{VECTOR:x}": [False, False]},
        },
        # The injection removes the event before the request: E1 is lost.
        "injection_pop_before_request": {
            "phase1_queue_len": len(PHASE1) - 1,
            "wakeups": 2,
            "counter": 2,
            "time_ns": FUTURE_NS,
            "coalesced": 1,
        },
    }


def validate_image(elf: bytes, raw: bytes) -> dict:
    entry, segments = parse_elf_load_segments(elf)
    code_start, _ = elf_symbol(elf, "_start")
    handler_gpa, _ = elf_symbol(elf, "stage4_queue_handler")
    code_end, _ = elf_symbol(elf, "stage4_code_end")
    if entry != CODE_GPA or code_start != CODE_GPA:
        raise BuildError(f"entry is 0x{entry:x}, expected 0x{CODE_GPA:x}")
    entry_bytes = entry_code()
    handler_bytes = handler_code()
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
        raise BuildError("entry code does not match the fixed profile")
    if at(handler_gpa, len(handler_bytes)) != handler_bytes:
        raise BuildError("handler code does not match the fixed profile")
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
    fixture = root / "tests/fixtures/guest/stage4/interrupt_queue"
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    commands = [
        [str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp", "-ffreestanding",
         "-fno-pic", "-fno-pie", "-mno-red-zone", "-c", str(fixture / "entry.S"),
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
    layout = validate_image(elf, raw)
    (out / "probe.bin").write_bytes(raw)
    (out / "probe.unpadded.bin").unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    manifest = {
        "schema": "stage4-interrupt-queue-probe-build-v1",
        "profile": "Stage4InterruptQueueProbeV1",
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
        "expected": expected_evidence(),
        "artifacts": {
            "probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"output: {out}")
    print(f"raw sha256: {manifest['artifacts']['probe.bin']['sha256']}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    try:
        return build(args)
    except (BuildError, OSError) as error:
        print(f"build_stage4_interrupt_queue_probe: error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
