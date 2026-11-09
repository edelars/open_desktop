#!/usr/bin/env python3
"""Build and validate the freestanding S4-27i4a paged STI/HLT wake probe image."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_stage4_flat_probe import (  # noqa: E402
    BuildError,
    output_directory,
    parse_elf_load_segments,
    resolve_executable,
    run,
    rust_toolchain,
    sha256,
)

IMAGE_BASE = 0x1000
IMAGE_SIZE = 0xA000
VECTOR = 0x41
# Leaf PTEs start present and writable with A and D clear; upper levels have A preset.
ENTRIES = {
    0x1000: 0x2023,
    0x2000: 0x3023,
    0x3010: 0x4023,  # GVA 0x400000 code table
    0x3018: 0x5023,  # GVA 0x600000 IDT/GDT/APIC/stack table
    0x4008: 0x6003,  # GVA 0x401000 main code
    0x4010: 0x7003,  # GVA 0x402000 interrupt handler
    0x5000: 0x8003,  # GVA 0x600000 IDT
    0x5008: 0x9003,  # GVA 0x601000 GDT
    0x5010: 0xFEE00003,  # GVA 0x602000 Local APIC page
    0x5800: 0xA003,  # GVA 0x700000 stack page
}
MAIN_CODE = bytes.fromhex("fb" "f4" "f4")  # STI; HLT; HLT
HANDLER_CODE = bytes.fromhex("8903" "48cf")  # MOV [RBX],EAX (EOI); IRETQ
HANDLER_GVA = 0x402000
MAIN_CODE_GPA = 0x6000
HANDLER_CODE_GPA = 0x7000
IDT_GPA = 0x8000
GDT_GPA = 0x9000
STACK_GPA = 0xA000
GATE_OFFSET = VECTOR * 16
# Interrupt gate 0x41: target 0x402000, CS 0x08, IST 0, type 0x8e (P=1, DPL 0).
GATE = (
    (HANDLER_GVA & 0xFFFF).to_bytes(2, "little")
    + (0x08).to_bytes(2, "little")
    + bytes([0, 0x8E])
    + ((HANDLER_GVA >> 16) & 0xFFFF).to_bytes(2, "little")
    + (HANDLER_GVA >> 32).to_bytes(4, "little")
    + bytes(4)
)
GDT = bytes(8) + bytes.fromhex("ffff0000009baf00") + bytes.fromhex("ffff00000093cf00")
# (vaddr, paddr, filesz, flags) of the five PT_LOAD segments.
SEGMENTS = [
    (0x1000, 0x1000, 0x5000, 6),
    (0x401000, MAIN_CODE_GPA, len(MAIN_CODE), 5),
    (0x402000, HANDLER_CODE_GPA, 4, 5),
    (0x600000, IDT_GPA, GATE_OFFSET + len(GATE), 6),
    (0x601000, GDT_GPA, len(GDT), 6),
]
# Expected leaf PTEs after the full case (SDM Vol. 3A 4.8: A on use, D on the first write).
EXPECTED_PTES = {
    "main": {"pte_gpa": "0x4008", "initial": "0x6003", "final": "0x6023", "bits": "A"},
    "handler": {"pte_gpa": "0x4010", "initial": "0x7003", "final": "0x7023", "bits": "A"},
    "idt": {"pte_gpa": "0x5000", "initial": "0x8003", "final": "0x8023", "bits": "A"},
    "gdt": {"pte_gpa": "0x5008", "initial": "0x9003", "final": "0x9023", "bits": "A"},
    "lapic": {"pte_gpa": "0x5010", "initial": "0xfee00003", "final": "0xfee00063", "bits": "A,D"},
    "stack": {"pte_gpa": "0x5800", "initial": "0xa003", "final": "0xa063", "bits": "A,D"},
}
# Expected guest facts, derived from the Intel SDM and the S4-27h image, not from a run of the code:
# - Vol. 2B STI: IF := 1; when IF was 0 before, maskable interrupts are inhibited until after the
#   next instruction (interrupt shadow); with IF already 1 there is no inhibition.
# - Vol. 3A 6.8 / Vol. 2A HLT: HLT stops instruction execution until an interrupt; it retires, so the
#   saved RIP of the waking interrupt is the instruction after HLT. The shadow ends with the
#   instruction that follows STI (HLT), so the wake is not blocked.
# - Vol. 3A 6.14.2/6.14.3 (64-bit frame): RSP is aligned down to 16 (0x700ff8 -> 0x700ff0), SS, RSP,
#   RFLAGS, CS, RIP are pushed (five qwords, no error code): new RSP 0x700fc8; an interrupt gate
#   clears IF in the live RFLAGS (0x202 -> 0x002) and the saved RFLAGS keeps IF.
# - Vol. 3A 13.8.4 and 13.9: IRR -> ISR on acknowledge, EOI write of 0 clears the highest ISR bit.
# - Vol. 3A 4.8: A on use, D on the first write; a supervisor write to a read-only page with
#   CR0.WP=1 is a #PF with P|W (error code 3) after the walk has set A.
SCENARIOS = {
    "wake_from_hlt": {
        "initial_rflags": "0x2",
        "steps": ["sti", "hlt", "idle_poll", "wake", "eoi", "iretq", "resume_hlt"],
        "retired_instructions": 5,
        # (rip, rsp, rflags, halted, shadow, irr, isr) after each step.
        "after": [
            ["0x401001", "0x700ff8", "0x202", False, True, False, False],
            ["0x401002", "0x700ff8", "0x202", True, False, False, False],
            ["0x401002", "0x700ff8", "0x202", True, False, False, False],
            ["0x402000", "0x700fc8", "0x2", False, False, False, True],
            ["0x402002", "0x700fc8", "0x2", False, False, False, False],
            ["0x401002", "0x700ff8", "0x202", False, False, False, False],
            ["0x401003", "0x700ff8", "0x202", True, False, False, False],
        ],
        "frame_low_to_high": ["rip=0x401002", "cs=0x08", "rflags=0x202", "rsp=0x700ff8", "ss=0x10"],
        "new_rsp": "0x700fc8",
    },
    "boundary_before_hlt": {
        "initial_rflags": "0x202",
        "steps": ["sti", "entry", "eoi", "iretq", "hlt", "idle_poll"],
        "retired_instructions": 4,
        "after": [
            ["0x401001", "0x700ff8", "0x202", False, False, False, False],
            ["0x402000", "0x700fc8", "0x2", False, False, False, True],
            ["0x402002", "0x700fc8", "0x2", False, False, False, False],
            ["0x401001", "0x700ff8", "0x202", False, False, False, False],
            ["0x401002", "0x700ff8", "0x202", True, False, False, False],
            ["0x401002", "0x700ff8", "0x202", True, False, False, False],
        ],
        "frame_low_to_high": ["rip=0x401001", "cs=0x08", "rflags=0x202", "rsp=0x700ff8", "ss=0x10"],
        "new_rsp": "0x700fc8",
    },
}
# Wake scenario, state right after the failing step (step 4); the CPU stays halted at 0x401002.
EXPECTED_INJECTIONS = {
    "idt_not_present": {
        "stop": "OutsideProfile(InvalidGate)",
        "steps": 4,
        "rip": "0x401002",
        "rsp": "0x700ff8",
        "rflags": "0x202",
        "halted": True,
        "irr": True,
        "isr": False,
        "ptes": {"main": "0x6023", "handler": "0x7003", "idt": "0x8023", "gdt": "0x9003",
                 "lapic": "0xfee00003", "stack": "0xa003"},
    },
    "stack_read_only": {
        "stop": "DeliveryAborted(StackWrite(OldSs), #PF P|W at 0x700fe8)",
        "steps": 4,
        "rip": "0x401002",
        "rsp": "0x700ff8",
        "rflags": "0x202",
        "halted": True,
        "irr": True,
        "isr": False,
        "cr2": None,
        "ptes": {"main": "0x6023", "handler": "0x7003", "idt": "0x8023", "gdt": "0x9023",
                 "lapic": "0xfee00003", "stack": "0xa021"},
    },
}
LAYOUT = {
    "entry_gva": "0x401000",
    "vector": f"0x{VECTOR:02x}",
    "main_code": {"gva": "0x401000", "gpa": "0x6000", "bytes": MAIN_CODE.hex(), "raw_offset": "0x5000"},
    "handler_code": {"gva": "0x402000", "gpa": "0x7000", "bytes": HANDLER_CODE.hex(), "raw_offset": "0x6000"},
    "idt": {
        "gva": "0x600000",
        "gpa": "0x8000",
        "gate_vector": VECTOR,
        "gate_offset": f"0x{GATE_OFFSET:x}",
        "gate": GATE.hex(),
        "limit": f"0x{GATE_OFFSET + len(GATE) - 1:x}",
        "raw_offset": "0x7000",
    },
    "gdt": {"gva": "0x601000", "gpa": "0x9000", "bytes": GDT.hex(), "raw_offset": "0x8000"},
    "stack": {"gva": "0x700000", "gpa": "0xa000", "initial_rsp": "0x700ff8", "raw_offset": "0x9000"},
    "mappings": [
        {"gva": "0x401000", "gpa": "0x6000", "access": "supervisor-rw-x", "pte_gpa": "0x4008"},
        {"gva": "0x402000", "gpa": "0x7000", "access": "supervisor-rw-x", "pte_gpa": "0x4010"},
        {"gva": "0x600000", "gpa": "0x8000", "access": "supervisor-rw", "pte_gpa": "0x5000"},
        {"gva": "0x601000", "gpa": "0x9000", "access": "supervisor-rw", "pte_gpa": "0x5008"},
        {"gva": "0x602000", "gpa": "0xfee00000", "access": "supervisor-rw-mmio", "pte_gpa": "0x5010"},
        {"gva": "0x700000", "gpa": "0xa000", "access": "supervisor-rw", "pte_gpa": "0x5800"},
    ],
    "eoi": {"rbx_gva": "0x6020b0", "physical": "0xfee000b0", "value": "0x0"},
    "expected_ptes": EXPECTED_PTES,
    "scenarios": SCENARIOS,
    "expected_injections": EXPECTED_INJECTIONS,
}


def expected_image() -> bytes:
    image = bytearray(IMAGE_SIZE)
    for address, value in ENTRIES.items():
        image[address - IMAGE_BASE : address - IMAGE_BASE + 8] = value.to_bytes(8, "little")
    for gpa, code in (
        (MAIN_CODE_GPA, MAIN_CODE),
        (HANDLER_CODE_GPA, HANDLER_CODE),
        (IDT_GPA + GATE_OFFSET, GATE),
        (GDT_GPA, GDT),
    ):
        image[gpa - IMAGE_BASE : gpa - IMAGE_BASE + len(code)] = code
    return bytes(image)


def validate_image(elf: bytes, raw: bytes) -> None:
    entry, segments = parse_elf_load_segments(elf)
    if entry != 0x401000 or len(segments) != len(SEGMENTS):
        raise BuildError("invalid entry or segment count")
    for segment, (vaddr, paddr, filesz, flags) in zip(segments, SEGMENTS, strict=True):
        if (segment["vaddr"], segment["paddr"], segment["filesz"], segment["flags"]) != (
            vaddr,
            paddr,
            filesz,
            flags,
        ):
            raise BuildError(f"unexpected segment {segment!r}")
        start = paddr - IMAGE_BASE
        if raw[start : start + filesz] != elf[segment["offset"] : segment["offset"] + filesz]:
            raise BuildError("raw segment does not match ELF")
    if len(raw) != IMAGE_SIZE:
        raise BuildError("image size is not the fixed profile size")
    if raw != expected_image():
        raise BuildError("image does not match the fixed paged STI/HLT profile")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/paged_sti_hlt"
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    commands = [
        [str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp",
         "-ffreestanding", "-fno-pic", "-fno-pie", "-mno-red-zone", "-c",
         str(fixture / "entry.S"), "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T",
         str(fixture / "layout.ld"), "-o", str(out / "paged_sti_hlt_probe.elf"),
         str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00",
         str(out / "paged_sti_hlt_probe.elf"), str(out / "paged_sti_hlt_probe.unpadded.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "paged_sti_hlt_probe.elf").read_bytes()
    unpadded = out / "paged_sti_hlt_probe.unpadded.bin"
    raw = unpadded.read_bytes()
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"objcopy image exceeds fixed size: {len(raw):#x}")
    raw = raw + bytes(IMAGE_SIZE - len(raw))
    validate_image(elf, raw)
    (out / "paged_sti_hlt_probe.bin").write_bytes(raw)
    unpadded.unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    manifest = {
        "schema": "stage4-paged-sti-hlt-probe-build-v1",
        "profile": "Stage4PagedStiHltProbeV1",
        "source_revision": revision,
        "layout": LAYOUT,
        "source": {name: sha256((fixture / name).read_bytes()) for name in ("entry.S", "layout.ld")},
        "target": "x86_64-unknown-none-elf",
        "toolchain": {
            "clang": clang_version,
            "linker": linker_version,
            "objcopy": objcopy_version.splitlines()[0] if objcopy_version else "",
            "rustc": rustc_version.splitlines()[0],
            "host": host,
        },
        "commands": commands,
        "artifacts": {
            "paged_sti_hlt_probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "paged_sti_hlt_probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"raw sha256: {manifest['artifacts']['paged_sti_hlt_probe.bin']['sha256']}; output: {out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
