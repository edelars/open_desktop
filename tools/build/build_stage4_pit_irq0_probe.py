#!/usr/bin/env python3
"""Build and validate the freestanding Stage4PitIrq0ProbeV1 kernel image (S4-29f).

The image is a flat identity-mapped Long64/CPL0 guest for PIT channel 0 -> OUT0 ->
IOAPIC INTIN2 (GSI 2) -> Local APIC: GDT, an IDT with one gate (vector 0x60), one code
page and a counter dword at 0x5000 (written by the handler). The loader installs IDTR,
so the image has no IDTR page. The raw image is loaded at GPA 0x1000 and is exactly
0x4000 bytes. The guest software-enables the Local APIC, initialises the master 8259
(vector base 0x30, everything masked), writes IOAPIC entry 2 (vector 0x60, edge,
unmasked), programs PIT channel 0 twice (control 0x34 = LSB+MSB, mode 2, binary; counts
1790 and then 1193: the second program replaces the first) and runs `STI; loop: HLT;
JMP loop`. The handler counts in RAM; after the third interrupt it reads the PIC IRR
(OCW3 0x0A, IN 0x20) and shuts channel 0 down (control word 0x30); then it writes the
EOI and returns with IRETQ.

The manifest records the expected evidence, derived from the PIT formula of S4-29c and
the rules of the event bridge (never from the interpreter's output):

* PIT counting: the input runs at 1_193_182 Hz from virtual time 0; the tick index at
  time t is T(t) = floor(t * 1_193_182 / 1e9). Instructions take 0 ns, so the guest
  loads channel 0 at time 0 (tick 0). Mode 2 with reload N: OUT0 rises at ticks k * N
  (k >= 1; OUT is low for one tick before each). Mode 0: one rising edge at tick N.
* Event deadline: the first nanosecond t with T(t) >= E for the edge at tick E, that is
  ceil(E * 1e9 / 1_193_182). The halted CPU's clock jumps to the next deadline (idle
  advance), so after the k-th wake the time is exactly that deadline. With N = 1193 the
  edges are due at 999_848, 1_999_695 and 2_999_543 ns; the replaced first program
  (count 1790) would have been due at 1_500_191 ns.
* Bridge: every change of channel 0 (control word, count, shutdown, reset) drops the
  pending events (new generation) and schedules the next edge. After an edge is
  delivered the next one is scheduled. Five scheduled events (first program, d1..d4),
  three delivered, the shutdown (control word 0x30) in the third handler drops d4;
  five generation changes; two events dropped (the first program's and d4).
* OUT0 reaches INTIN2 only; the 8259 pair has no input lines, so the PIC IRR reads 0
  and no vector of the PIC IR0 (0x30) rises.
* Pump quanta (one per step): the guest executes 42 instructions up to and including
  STI; HLT is the 43rd step; each idle advance is one step. Injections (derived by
  counting the steps): masked entry 2 loses every edge (budget ends the run at
  d_(budget-43)); a missing EOI blocks the second entry (one wake, then the budget ends
  the run at d_(budget-56)); no generation change delivers the replaced program's edge.
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
VECTOR = 0x60
PIC_VECTOR = 0x30
GDT = struct.pack("<QQQ", 0, 0x00AF9B000000FFFF, 0x00CF93000000FFFF)
PIT_HZ = 1_193_182
COUNT_FIRST = 1790
COUNT_SECOND = 1193
DEFAULT_INTERRUPTS = 3
CONTROL_MODE2 = 0x34  # channel 0, LSB+MSB, mode 2, binary
CONTROL_MODE0 = 0x30  # channel 0, LSB+MSB, mode 0, binary (also the shutdown word)
# Instruction encodings (Intel SDM Vol. 2): MOV r32,imm32 = B8+r; MOV r/m32,r32 = 89 /r
# (ModRM 0x87 = [RDI+disp32],EAX; 0x06 = [RSI],EAX; 0x46 = [RSI+disp8],EAX); MOV AL,imm8 B0;
# MOV EDX,imm32 BA; OUT DX,AL EE; IN AL,DX EC; MOV EAX,[RSI] 8B 06; ADD r/m32,imm8 83 /0
# (C0 = EAX); CMP r/m32,imm8 83 /7 (F8 = EAX); JNE rel8 75; STI FB; HLT F4; JMP rel8 EB;
# IRETQ 48 CF.
MOV_EDI = bytes.fromhex("bf0000e0fe")  # MOV EDI,0xFEE00000 (Local APIC)
MOV_ESI_IOAPIC = bytes.fromhex("be0000c0fe")  # MOV ESI,0xFEC00000 (IOAPIC)
MOV_ESI_COUNTER = bytes.fromhex("be00500000")  # MOV ESI,0x5000


def mov_eax(value: int) -> bytes:
    return b"\xb8" + struct.pack("<I", value)


def store_eax(disp: int) -> bytes:
    return bytes.fromhex("8987") + struct.pack("<I", disp)  # MOV [RDI+disp],EAX


def write_reg(disp: int, value: int) -> bytes:
    return mov_eax(value) + store_eax(disp)


def out8(port: int, value: int) -> bytes:
    """MOV EDX,port; MOV AL,value; OUT DX,AL (three instructions)."""
    return b"\xba" + struct.pack("<I", port) + bytes([0xB0, value, 0xEE])


def program_channel0(control: int, count: int) -> bytes:
    return out8(0x43, control) + out8(0x40, count & 0xFF) + out8(0x40, count >> 8)


PIC_INIT = [(0x20, 0x11), (0x21, 0x30), (0x21, 0x04), (0x21, 0x01), (0x21, 0xFF)]
UNMASK_VALUE = mov_eax(VECTOR)  # entry 2 low dword: edge, unmasked, fixed, vector 0x60


def entry_prefix() -> bytes:
    """Up to (not including) the IOAPIC entry-2 value write."""
    return (
        MOV_EDI
        + write_reg(0xF0, 0x1FF)  # SVR = 0x1FF: software enable
        + b"".join(out8(port, value) for port, value in PIC_INIT)
        + MOV_ESI_IOAPIC
        + mov_eax(0x14)  # IOREGSEL = entry 2 low dword
        + bytes.fromhex("8906")  # MOV [RSI],EAX
    )


def entry_code(control: int) -> bytes:
    return (
        entry_prefix()
        + UNMASK_VALUE
        + bytes.fromhex("894610")  # MOV [RSI+0x10],EAX (IOWIN)
        + program_channel0(control, COUNT_FIRST)
        + program_channel0(control, COUNT_SECOND)  # the reprogram
        + bytes.fromhex("fbf4ebfd")  # STI; loop: HLT; JMP loop (rel8 = -3)
    )


# 1 (MOV EDI) + 2 (SVR) + 5 OUTs * 3 + 5 (IOAPIC) + 6 OUTs * 3 + 1 (STI).
INSTRUCTIONS_TO_STI = 1 + 2 + 5 * 3 + 5 + 6 * 3 + 1
STEPS_TO_HLT = INSTRUCTIONS_TO_STI + 1


def shutdown_block() -> bytes:
    return (
        out8(0x20, 0x0A)  # OCW3: read IRR
        + b"\xba" + struct.pack("<I", 0x20)  # MOV EDX,0x20
        + b"\xec"  # IN AL,DX
        + out8(0x43, CONTROL_MODE0)  # shutdown of channel 0
    )


def handler_code(interrupts: int) -> bytes:
    stop = shutdown_block()
    return (
        MOV_EDI
        + MOV_ESI_COUNTER
        + bytes.fromhex("8b06")  # MOV EAX,[RSI]
        + bytes.fromhex("83c001")  # ADD EAX,1
        + bytes.fromhex("8906")  # MOV [RSI],EAX
        + bytes.fromhex("83f8") + bytes([interrupts])  # CMP EAX,interrupts
        + bytes([0x75, len(stop)])  # JNE ack
        + stop
        + write_reg(0xB0, 0)  # EOI
        + bytes.fromhex("48cf")  # IRETQ
    )


def unmask_gpa() -> int:
    """Image GPA of the entry-2 value (`MOV EAX,0x60`); the MaskedEntry2 injection patches it."""
    return CODE_GPA + len(entry_prefix())


def eoi_gpa(control: int) -> int:
    """Image GPA of the EOI store `MOV [RDI+0xB0],EAX`; the NoEoi injection patches it."""
    handler = CODE_GPA + len(entry_code(control))
    return handler + len(handler_code(1)) - 2 - len(store_eax(0xB0))


# PIT formulas (Intel 8254 datasheet; project S4-29c).
def pit_ticks(now_ns: int) -> int:
    """Tick index of the PIT input clock at `now_ns`: floor(now_ns * f / 1e9)."""
    return now_ns * PIT_HZ // 1_000_000_000


def deadline_ns(tick: int) -> int:
    """First nanosecond whose tick index reaches `tick`: ceil(tick * 1e9 / f)."""
    return -(-tick * 1_000_000_000 // PIT_HZ)


def edge_ticks(control: int, count: int, edges: int) -> list[int]:
    """Rising edges of OUT0 after a load at tick 0: mode 2 at k * N, mode 0 at N only."""
    if control == CONTROL_MODE0:
        return [count][:edges]
    return [k * count for k in range(1, edges + 1)]


def expected_evidence(interrupts: int, control: int) -> dict:
    """Expected evidence; addresses follow from the encodings, the rest is PIT arithmetic."""
    entry = entry_code(control)
    hlt = CODE_GPA + len(entry) - 3  # HLT, then JMP rel8
    handler = CODE_GPA + len(entry)
    frame_rsp = STACK_TOP - 5 * 8
    mode2 = control == CONTROL_MODE2
    wakeups = interrupts if mode2 else 1
    ticks = edge_ticks(control, COUNT_SECOND, wakeups)
    wake_times = [deadline_ns(tick) for tick in ticks]
    first_program_due = deadline_ns(edge_ticks(control, COUNT_FIRST, 1)[0])
    ev = lambda name, vector=None: {"event": name} if vector is None else {"event": name, "vector": vector}  # noqa: E731
    events = [ev("sti"), ev("hlt")]
    for _ in range(wakeups):
        events += [ev("irr", VECTOR), {"event": "entry", "vector": VECTOR, "source": "halt_wake"},
                   ev("eoi", VECTOR), ev("iretq"), ev("hlt")]
    shuts_down = wakeups == interrupts
    # Scheduled: the first program, then one edge per wake, plus the next one while the
    # chain runs (mode 2 always schedules the next edge after a delivery; mode 0 never).
    scheduled = 1 + wakeups + (1 if mode2 else 0)
    # Generation changes: control word, count (x2 programs), plus the shutdown.
    generation_changes = 4 + (1 if shuts_down else 0)
    # Dropped: the first program's event, plus the pending next edge at the shutdown.
    cancelled = 1 + (1 if shuts_down and mode2 else 0)
    return {
        "pit": {
            "input_hz": PIT_HZ,
            "control_word": f"0x{control:02x}",
            "mode": 2 if mode2 else 0,
            "count_first_program": COUNT_FIRST,
            "count": COUNT_SECOND,
            "formula": "edge at tick E is due at ceil(E * 1e9 / 1193182) ns",
            "edge_ticks": ticks,
            "first_program_would_be_due_ns": first_program_due,
        },
        "ioapic": {"entry": 2, "vector": f"0x{VECTOR:x}", "trigger": "edge", "masked": False},
        "interrupts_before_shutdown": interrupts,
        "wakeups": wakeups,
        "counter": wakeups,
        "counter_gpa": f"0x{COUNTER_GPA:x}",
        "wake_times_ns": wake_times,
        "time_ns": wake_times[-1],
        "period_ns": deadline_ns(COUNT_SECOND),
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
        "bridge": {
            "edges": wakeups,
            "scheduled": scheduled,
            "generation_changes": generation_changes,
            "cancelled": cancelled,
        },
        "pic": {
            "pic_irr_read": 0,
            "pic_vector": f"0x{PIC_VECTOR:x}",
            "pic_vector_state": [False, False],
            "master_imr": "0xff",
        },
        "final": {
            "exit": "idle_halt",
            "rip": f"0x{hlt + 1:x}",
            "rsp": f"0x{STACK_TOP:x}",
            "rflags": "0x202",
            "vector": {f"0x{VECTOR:x}": [False, False]},
        },
    }


def injection_evidence(budget: int) -> dict:
    """Distinguishing evidence (stop, wakeups, counter, time) of the injections at `budget` steps."""
    count = COUNT_SECOND
    # Masked entry 2: step 43 HLT, step 43 + m has the clock at d_m; no wake, no counter.
    masked_m = budget - STEPS_TO_HLT
    # Missing EOI: 43 HLT, 44 idle advance to d1, 45 wake, 46..55 the handler (10
    # instructions without the shutdown block), 56 JMP, 57 HLT; then each step moves the
    # clock to the next edge: after step 58 + j it is d_(2 + j).
    first_hlt_after_wake = STEPS_TO_HLT + 1 + 1 + 10 + 1 + 1
    noeoi_m = 2 + (budget - first_hlt_after_wake - 1)
    # No generation change: the first program's edge (tick 1790) is delivered between
    # d1 and d2; the counter reaches 3 at d2 (the shutdown drops nothing), d3 is delivered
    # as the fourth interrupt.
    nogen_times = sorted(
        [deadline_ns(k * count) for k in (1, 2, 3)] + [deadline_ns(COUNT_FIRST)]
    )
    return {
        "budget": budget,
        "masked_entry2": {
            "stop": "budget", "wakeups": 0, "counter": 0,
            "time_ns": deadline_ns(masked_m * count), "edges": masked_m - 1,
        },
        "no_eoi": {
            "stop": "budget", "wakeups": 1, "counter": 1,
            "time_ns": deadline_ns(noeoi_m * count),
            "edges": budget - first_hlt_after_wake, "vector_state": [True, True],
        },
        "no_generation": {
            "stop": "idle_halt", "wakeups": 4, "counter": 4,
            "time_ns": nogen_times[-1], "wake_times_ns": nogen_times,
            "generation_changes": 0,
        },
    }


def validate_image(elf: bytes, raw: bytes, args: argparse.Namespace) -> dict:
    entry, segments = parse_elf_load_segments(elf)
    code_start, _ = elf_symbol(elf, "_start")
    handler_gpa, _ = elf_symbol(elf, "stage4_pit_handler")
    code_end, _ = elf_symbol(elf, "stage4_code_end")
    if entry != CODE_GPA or code_start != CODE_GPA:
        raise BuildError(f"entry is 0x{entry:x}, expected 0x{CODE_GPA:x}")
    entry_bytes = entry_code(args.control)
    handler_bytes = handler_code(args.interrupts)
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
        "unmask_value_gpa": f"0x{unmask_gpa():x}",
        "eoi_store_gpa": f"0x{eoi_gpa(args.control):x}",
        "load_gpa": f"0x{IMAGE_BASE:x}",
        "image_size": f"0x{IMAGE_SIZE:x}",
        "segments": [
            {"paddr": f"0x{p:x}", "filesz": f"0x{s:x}", "flags": f"0x{f:x}"} for _, p, s, f in expected
        ],
    }


def build(args: argparse.Namespace) -> int:
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/pit_irq0"
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    commands = [
        [str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp", "-ffreestanding",
         "-fno-pic", "-fno-pie", "-mno-red-zone",
         f"-DPROBE_INTERRUPTS={args.interrupts}", f"-DPROBE_CONTROL={args.control}",
         "-c", str(fixture / "entry.S"), "-o", str(out / "entry.o")],
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
    default_profile = args.interrupts == DEFAULT_INTERRUPTS and args.control == CONTROL_MODE2
    manifest = {
        "schema": "stage4-pit-irq0-probe-build-v1",
        "profile": "Stage4PitIrq0ProbeV1",
        "accepted_profile": default_profile,
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
        "expected": expected_evidence(args.interrupts, args.control),
        "injections": injection_evidence(200),
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
    parser.add_argument("--interrupts", type=int, default=DEFAULT_INTERRUPTS,
                        help="interrupts before the handler shuts channel 0 down (1..127)")
    parser.add_argument("--control", type=lambda v: int(v, 0), default=CONTROL_MODE2,
                        choices=[CONTROL_MODE2, CONTROL_MODE0],
                        help="channel 0 control word: 0x34 (mode 2) or 0x30 (mode 0)")
    args = parser.parse_args()
    if not 1 <= args.interrupts <= 127:
        parser.error("--interrupts must be between 1 and 127")
    try:
        return build(args)
    except (BuildError, OSError) as error:
        print(f"build_stage4_pit_irq0_probe: error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
